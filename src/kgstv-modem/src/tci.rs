//! TCI (Transceiver Control Interface) client — radio control plus RX and TX audio over a single
//! WebSocket connection, as served by ExpertSDR2/3 (SunSDR), piHPSDR, deskHPSDR and others.
//!
//! Protocol facts used here (TCI Protocol 2.1 PDF, cross-checked against piHPSDR's `tci.c` /
//! `tci_audio.c` server implementation):
//! - Text commands are ASCII `name:arg1,arg2;` (case-insensitive); several may arrive in one
//!   WebSocket message. The server sends its initial state on connect, ending with `ready;`.
//! - Audio travels in binary WebSocket messages: a 64-byte header of 16 little-endian `u32`
//!   (`receiver, sample_rate, format, codec, crc, length, type, channels, count, reserv[7]`)
//!   followed by the samples. `length` is the TOTAL number of sample values in the block
//!   (L and R interleaved for stereo), not frames per channel.
//! - Stream types: 0 IQ, 1 RX audio, 2 TX audio, 3 TX_CHRONO, 4 line-out.
//!   Sample formats: 0 int16, 1 int24, 2 int32, 3 float32. Defaults: 48 kHz, float32, 2 channels.
//! - RX audio: `audio_samplerate:48000; audio_stream_sample_type:float32;
//!   audio_stream_channels:2; audio_start:0;` — the server then streams type-1 blocks.
//! - TX audio: after `trx:0,true,tci;` the server sends TX_CHRONO blocks (header only) and the
//!   client answers each one with a TX_AUDIO_STREAM block of exactly `length` values. Answering
//!   with zeros when no audio is ready is the spec's preferred behaviour.
//!
//! One background thread owns the socket; the GUI talks to it through a cloneable `TciClient`
//! handle. RX audio is fed into the same `RingBuffer` a sound-card `LiveCapture` uses, so the
//! whole progressive-decode/eye-diagram path works unchanged with a TCI source.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tungstenite::{Message, WebSocket};

use crate::live::{push_downmixed, resample_linear, CaptureSource, LiveCapture, RingBuffer};

/// Default TCI server port. ExpertSDR3 uses 50001; ExpertSDR2 and some other TCI servers use
/// 40001, which can be entered in the GUI's Port field.
pub const TCI_DEFAULT_PORT: u16 = 50001;
/// Sample rate requested for both RX and TX audio streams.
pub const TCI_AUDIO_SAMPLE_RATE: f64 = 48000.0;

const HEADER_LEN: usize = 64;
const STREAM_RX_AUDIO: u32 = 1;
const STREAM_TX_AUDIO: u32 = 2;
const STREAM_TX_CHRONO: u32 = 3;
const FORMAT_INT16: u32 = 0;
const FORMAT_INT24: u32 = 1;
const FORMAT_INT32: u32 = 2;
const FORMAT_FLOAT32: u32 = 3;

/// Silence sent after the last TX sample before unkeying, so audio still buffered in the radio
/// (piHPSDR prebuffers ~85 ms; ExpertSDR's TX_STREAM_AUDIO_BUFFERING defaults to 50 ms) is
/// transmitted instead of being cut off by PTT release.
const TX_TAIL: Duration = Duration::from_millis(500);
/// If the radio has not asked for any TX audio this long after keying, it does not support (or
/// has not enabled) TCI TX audio — give up with a clear error instead of holding PTT forever.
const TX_CHRONO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TciState {
    #[default]
    Connecting,
    Connected,
    /// Not connected; the string is the reason (empty after a user-requested disconnect).
    Disconnected(String),
}

/// Radio state as last reported by the TCI server.
#[derive(Debug, Clone, Default)]
pub struct TciStatus {
    pub state: TciState,
    /// `protocol:` line, e.g. "ExpertSDR3,2.0".
    pub protocol: Option<String>,
    /// `device:` line, e.g. "SunSDR2QRP".
    pub device: Option<String>,
    /// The server finished sending its initial state (`ready;`).
    pub ready: bool,
    /// VFO A frequency of receiver 0, Hz.
    pub frequency_hz: Option<u64>,
    /// Modulation of receiver 0, lower case (e.g. "usb", "digu").
    pub modulation: Option<String>,
    /// Receiver 0 is transmitting (as reported by the server's `trx:` notifications).
    pub transmitting: bool,
    /// Sample rate of the RX audio blocks actually received.
    pub rx_audio_rate: Option<u32>,
}

/// Where received RX audio goes while a TCI `LiveCapture` exists.
struct RxSink {
    buffer: Arc<Mutex<RingBuffer>>,
    gain: Arc<Mutex<f32>>,
    level: Arc<Mutex<f32>>,
}

#[derive(Default)]
struct TxState {
    active: bool,
    queue: VecDeque<f32>,
    /// Number of TX_CHRONO requests answered since keying.
    chrono_count: u64,
}

struct Shared {
    status: Mutex<TciStatus>,
    rx_sink: Mutex<Option<RxSink>>,
    tx: Mutex<TxState>,
    shutdown: AtomicBool,
}

/// Cloneable handle to a TCI connection. The connection lives until `disconnect()` is called (or
/// the server goes away); dropping handles does not close it.
#[derive(Clone)]
pub struct TciClient {
    shared: Arc<Shared>,
    cmd: Sender<String>,
    url: String,
}

impl TciClient {
    /// Starts connecting to `ws://host:port` in the background and returns immediately; watch
    /// `status().state` for `Connected` / `Disconnected(reason)`.
    pub fn connect(host: &str, port: u16) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(TciStatus::default()),
            rx_sink: Mutex::new(None),
            tx: Mutex::new(TxState::default()),
            shutdown: AtomicBool::new(false),
        });
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let host = host.trim().to_string();
        let url = format!("ws://{host}:{port}");
        let shared_t = shared.clone();
        std::thread::spawn(move || {
            let reason = match open_socket(&host, port) {
                Ok(ws) => {
                    shared_t.status.lock().unwrap().state = TciState::Connected;
                    run_connection(ws, &shared_t, &cmd_rx)
                }
                Err(e) => e,
            };
            let mut st = shared_t.status.lock().unwrap();
            st.state = TciState::Disconnected(reason);
            st.transmitting = false;
            st.ready = false;
            shared_t.tx.lock().unwrap().active = false;
        });
        TciClient { shared, cmd: cmd_tx, url }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn status(&self) -> TciStatus {
        self.shared.status.lock().unwrap().clone()
    }

    pub fn is_connected(&self) -> bool {
        self.status().state == TciState::Connected
    }

    /// Closes the connection (unkeying first if transmitting).
    pub fn disconnect(&self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
    }

    /// Sends a raw TCI command; a trailing `;` is added if missing.
    pub fn send_command(&self, cmd: &str) {
        let mut c = cmd.trim().to_string();
        if !c.ends_with(';') {
            c.push(';');
        }
        let _ = self.cmd.send(c);
    }

    /// Tunes VFO A of receiver 0.
    pub fn set_frequency(&self, hz: u64) {
        self.send_command(&format!("vfo:0,0,{hz};"));
    }

    /// Sets the modulation of receiver 0 (e.g. "usb", "digu").
    pub fn set_modulation(&self, mode: &str) {
        self.send_command(&format!("modulation:0,{};", mode.trim().to_lowercase()));
    }

    fn send_audio_format(&self) {
        self.send_command(&format!("audio_samplerate:{};", TCI_AUDIO_SAMPLE_RATE as u32));
        self.send_command("audio_stream_sample_type:float32;");
        self.send_command("audio_stream_channels:2;");
    }

    /// Starts the radio's RX audio stream and returns a `LiveCapture` fed by it — a drop-in
    /// replacement for a sound-card capture. Dropping the capture stops the stream.
    pub fn start_rx_audio(&self, max_seconds: f64, initial_gain: f32) -> Result<LiveCapture, String> {
        if !self.is_connected() {
            return Err(format!("TCI not connected ({})", self.url));
        }
        let buffer = Arc::new(Mutex::new(RingBuffer::new((max_seconds * TCI_AUDIO_SAMPLE_RATE).round() as usize)));
        let gain = Arc::new(Mutex::new(initial_gain));
        let level = Arc::new(Mutex::new(0.0f32));
        *self.shared.rx_sink.lock().unwrap() =
            Some(RxSink { buffer: buffer.clone(), gain: gain.clone(), level: level.clone() });
        self.send_audio_format();
        self.send_command("audio_start:0;");
        let guard = TciRxGuard { shared: self.shared.clone(), cmd: self.cmd.clone() };
        Ok(LiveCapture::from_parts(TCI_AUDIO_SAMPLE_RATE, buffer, gain, level, CaptureSource::Tci(guard)))
    }

    /// Transmits `samples` (mono, -1.0..1.0, at `sample_rate` Hz) through the radio: keys PTT
    /// with TCI as the audio source, feeds the audio as the radio requests it, then unkeys.
    /// Blocks until done — call it from a background thread. `stop` aborts early (PTT is still
    /// released). Same contract as `live::play_blocking`.
    pub fn transmit_blocking(
        &self,
        samples: &[f64],
        sample_rate: f64,
        gain: f32,
        stop: Option<Arc<AtomicBool>>,
    ) -> Result<(), String> {
        if !self.is_connected() {
            return Err(format!("TCI not connected ({})", self.url));
        }
        let audio = if (sample_rate - TCI_AUDIO_SAMPLE_RATE).abs() > 0.5 {
            resample_linear(samples, sample_rate, TCI_AUDIO_SAMPLE_RATE)
        } else {
            samples.to_vec()
        };
        {
            let mut tx = self.shared.tx.lock().unwrap();
            tx.queue = audio.iter().map(|&v| (v * gain as f64).clamp(-1.0, 1.0) as f32).collect();
            tx.chrono_count = 0;
            tx.active = true;
        }
        self.send_audio_format();
        self.send_command("trx:0,true,tci;");

        let keyed_at = Instant::now();
        let mut drained_at: Option<Instant> = None;
        let result = loop {
            std::thread::sleep(Duration::from_millis(20));
            if stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
                break Ok(());
            }
            if !self.is_connected() {
                break Err("TCI connection lost while transmitting".to_string());
            }
            let (remaining, chronos) = {
                let tx = self.shared.tx.lock().unwrap();
                (tx.queue.len(), tx.chrono_count)
            };
            if chronos == 0 && keyed_at.elapsed() > TX_CHRONO_TIMEOUT {
                break Err(
                    "The radio did not request any TX audio after keying (no TX_CHRONO). Check that the \
                     TCI server supports TX audio and that TCI is allowed as the TX audio source."
                        .to_string(),
                );
            }
            if remaining == 0 {
                let t = *drained_at.get_or_insert_with(Instant::now);
                if t.elapsed() >= TX_TAIL {
                    break Ok(());
                }
            }
        };

        {
            let mut tx = self.shared.tx.lock().unwrap();
            tx.active = false;
            tx.queue.clear();
        }
        self.send_command("trx:0,false;");
        result
    }
}

/// Keeps the RX audio stream running while a TCI-fed `LiveCapture` exists.
pub(crate) struct TciRxGuard {
    shared: Arc<Shared>,
    cmd: Sender<String>,
}

impl Drop for TciRxGuard {
    fn drop(&mut self) {
        *self.shared.rx_sink.lock().unwrap() = None;
        let _ = self.cmd.send("audio_stop:0;".to_string());
    }
}

fn open_socket(host: &str, port: u16) -> Result<WebSocket<TcpStream>, String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("Cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("Cannot resolve {host}"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("Cannot connect to {host}:{port}: {e}"))?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();
    let (ws, _resp) = tungstenite::client(format!("ws://{host}:{port}/"), stream)
        .map_err(|e| format!("TCI WebSocket handshake failed: {e}"))?;
    // Short read timeout from here on: the loop polls the socket and the outgoing command queue.
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(10))).ok();
    Ok(ws)
}

/// Socket loop; returns the reason the connection ended.
fn run_connection(mut ws: WebSocket<TcpStream>, shared: &Shared, cmd_rx: &Receiver<String>) -> String {
    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            if shared.tx.lock().unwrap().active {
                let _ = ws.send(Message::text("trx:0,false;"));
            }
            if shared.rx_sink.lock().unwrap().is_some() {
                let _ = ws.send(Message::text("audio_stop:0;"));
            }
            let _ = ws.close(None);
            let _ = ws.flush();
            return String::new();
        }
        while let Ok(cmd) = cmd_rx.try_recv() {
            if let Err(e) = ws.send(Message::text(cmd)) {
                return format!("TCI send failed: {e}");
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => handle_text(shared, t.as_str()),
            Ok(Message::Binary(b)) => {
                if let Some(reply) = handle_binary(shared, &b) {
                    if let Err(e) = ws.send(Message::binary(reply)) {
                        return format!("TCI send failed: {e}");
                    }
                }
            }
            Ok(Message::Close(_)) => return "Closed by the TCI server".to_string(),
            Ok(_) => {} // ping/pong handled by tungstenite
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                // No data this tick; flush any pending pong/close frames.
                let _ = ws.flush();
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return "Closed by the TCI server".to_string();
            }
            Err(e) => return format!("TCI connection error: {e}"),
        }
    }
}

/// Parses one text message (possibly several `;`-terminated commands) and updates the status.
fn handle_text(shared: &Shared, text: &str) {
    let mut st = shared.status.lock().unwrap();
    for cmd in text.split(';') {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        let (name, args) = match cmd.split_once(':') {
            Some((n, a)) => (n.trim().to_lowercase(), a.split(',').map(|s| s.trim()).collect::<Vec<_>>()),
            None => (cmd.to_lowercase(), Vec::new()),
        };
        let arg = |i: usize| args.get(i).copied().unwrap_or("");
        match name.as_str() {
            "protocol" => st.protocol = Some(args.join(",")),
            "device" => st.device = Some(arg(0).to_string()),
            "ready" => st.ready = true,
            "vfo" if arg(0) == "0" && arg(1) == "0" => {
                if let Ok(f) = arg(2).parse::<f64>() {
                    st.frequency_hz = Some(f.round() as u64);
                }
            }
            "modulation" if arg(0) == "0" => st.modulation = Some(arg(1).to_lowercase()),
            "trx" if arg(0) == "0" => st.transmitting = arg(1).eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
}

fn header_u32(b: &[u8], index: usize) -> u32 {
    u32::from_le_bytes(b[index * 4..index * 4 + 4].try_into().unwrap())
}

/// Handles one binary stream block; returns a TX audio block to send back for TX_CHRONO.
fn handle_binary(shared: &Shared, b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < HEADER_LEN {
        return None;
    }
    match header_u32(b, 6) {
        STREAM_RX_AUDIO => {
            handle_rx_audio(shared, b);
            None
        }
        STREAM_TX_CHRONO => build_tx_reply(shared, b),
        _ => None, // IQ, line-out: not used
    }
}

fn handle_rx_audio(shared: &Shared, b: &[u8]) {
    if header_u32(b, 0) != 0 {
        return; // only receiver 0
    }
    let sink_guard = shared.rx_sink.lock().unwrap();
    let Some(sink) = sink_guard.as_ref() else { return };
    let format = header_u32(b, 2);
    let bytes_per = match format {
        FORMAT_INT16 => 2,
        FORMAT_INT24 => 3,
        FORMAT_INT32 | FORMAT_FLOAT32 => 4,
        _ => return,
    };
    let channels = match header_u32(b, 7) {
        0 => 2, // pre-1.5 servers leave the field 0; the default stream is stereo
        c => c as usize,
    };
    // `length` is the total value count; never trust it beyond what the block actually holds.
    let n = (header_u32(b, 5) as usize).min((b.len() - HEADER_LEN) / bytes_per);
    let data = &b[HEADER_LEN..HEADER_LEN + n * bytes_per];
    let values: Vec<f64> = data
        .chunks_exact(bytes_per)
        .map(|s| match format {
            FORMAT_INT16 => i16::from_le_bytes([s[0], s[1]]) as f64 / 32768.0,
            FORMAT_INT24 => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f64 / 8_388_608.0,
            FORMAT_INT32 => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64 / 2_147_483_648.0,
            _ => f32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64,
        })
        .collect();
    // Use the left channel only: in DIGU/DIGL with 2 channels the stream is complex (I/Q) and
    // I alone is the ordinary audio; for normal stereo audio both channels are identical.
    let left: Vec<f64> = values.chunks(channels).map(|c| c[0]).collect();
    push_downmixed(&sink.buffer, &sink.gain, &sink.level, &left, 1, |v| v);
    drop(sink_guard);
    let rate = header_u32(b, 1);
    let mut st = shared.status.lock().unwrap();
    if st.rx_audio_rate != Some(rate) {
        st.rx_audio_rate = Some(rate);
    }
}

/// Answers a TX_CHRONO request with exactly `length` float32 values (zeros once our audio has run
/// out or when we are not transmitting through TCI).
fn build_tx_reply(shared: &Shared, chrono: &[u8]) -> Option<Vec<u8>> {
    let mut tx = shared.tx.lock().unwrap();
    if !tx.active {
        return None; // another source owns TX; the spec allows not answering
    }
    tx.chrono_count += 1;
    let length = header_u32(chrono, 5) as usize;
    let channels = match header_u32(chrono, 7) {
        0 => 2,
        c => c as usize,
    };
    if length == 0 || length > 1 << 16 {
        return None;
    }
    let frames = length / channels;
    let mut out = Vec::with_capacity(HEADER_LEN + length * 4);
    let fields = [
        header_u32(chrono, 0), // receiver
        TCI_AUDIO_SAMPLE_RATE as u32,
        FORMAT_FLOAT32,
        0, // codec
        0, // crc
        length as u32,
        STREAM_TX_AUDIO,
        channels as u32,
    ];
    for f in fields {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out.resize(HEADER_LEN, 0); // count + reserved
    for _ in 0..frames {
        let v = tx.queue.pop_front().unwrap_or(0.0);
        for _ in 0..channels {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out.resize(HEADER_LEN + length * 4, 0); // odd `length` with stereo: pad
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            status: Mutex::new(TciStatus::default()),
            rx_sink: Mutex::new(None),
            tx: Mutex::new(TxState::default()),
            shutdown: AtomicBool::new(false),
        }
    }

    fn header(receiver: u32, rate: u32, format: u32, length: u32, kind: u32, channels: u32) -> Vec<u8> {
        let mut h = Vec::new();
        for f in [receiver, rate, format, 0, 0, length, kind, channels] {
            h.extend_from_slice(&f.to_le_bytes());
        }
        h.resize(HEADER_LEN, 0);
        h
    }

    #[test]
    fn parses_initial_state_and_notifications() {
        let s = shared();
        handle_text(&s, "protocol:ExpertSDR3,2.0;device:SunSDR2QRP;");
        handle_text(&s, "vfo:0,0,14233000;vfo:0,1,7000000;modulation:0,USB;trx:0,true;ready;");
        let st = s.status.lock().unwrap();
        assert_eq!(st.protocol.as_deref(), Some("ExpertSDR3,2.0"));
        assert_eq!(st.device.as_deref(), Some("SunSDR2QRP"));
        assert_eq!(st.frequency_hz, Some(14_233_000)); // channel B (vfo:0,1) must not overwrite A
        assert_eq!(st.modulation.as_deref(), Some("usb"));
        assert!(st.transmitting);
        assert!(st.ready);
    }

    #[test]
    fn rx_audio_float32_stereo_takes_left_channel() {
        let s = shared();
        let buffer = Arc::new(Mutex::new(RingBuffer::new(1000)));
        *s.rx_sink.lock().unwrap() = Some(RxSink {
            buffer: buffer.clone(),
            gain: Arc::new(Mutex::new(1.0)),
            level: Arc::new(Mutex::new(0.0)),
        });
        // length = 6 TOTAL values = 3 stereo frames.
        let mut b = header(0, 48000, FORMAT_FLOAT32, 6, STREAM_RX_AUDIO, 2);
        for v in [0.5f32, -9.0, -0.25, -9.0, 0.125, -9.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        assert!(handle_binary(&s, &b).is_none());
        let got: Vec<f64> = buffer.lock().unwrap().data.iter().copied().collect();
        assert_eq!(got, vec![0.5, -0.25, 0.125]);
        assert_eq!(s.status.lock().unwrap().rx_audio_rate, Some(48000));
    }

    #[test]
    fn rx_audio_int16_mono_and_other_receiver_ignored() {
        let s = shared();
        let buffer = Arc::new(Mutex::new(RingBuffer::new(1000)));
        *s.rx_sink.lock().unwrap() = Some(RxSink {
            buffer: buffer.clone(),
            gain: Arc::new(Mutex::new(1.0)),
            level: Arc::new(Mutex::new(0.0)),
        });
        let mut other = header(1, 48000, FORMAT_INT16, 2, STREAM_RX_AUDIO, 1);
        other.extend_from_slice(&[0, 0x40, 0, 0x40]);
        handle_binary(&s, &other);
        assert!(buffer.lock().unwrap().data.is_empty());

        let mut b = header(0, 48000, FORMAT_INT16, 2, STREAM_RX_AUDIO, 1);
        b.extend_from_slice(&16384i16.to_le_bytes());
        b.extend_from_slice(&(-32768i16).to_le_bytes());
        handle_binary(&s, &b);
        let got: Vec<f64> = buffer.lock().unwrap().data.iter().copied().collect();
        assert_eq!(got, vec![0.5, -1.0]);
    }

    #[test]
    fn tx_chrono_reply_has_exactly_length_values_then_zeros() {
        let s = shared();
        {
            let mut tx = s.tx.lock().unwrap();
            tx.active = true;
            tx.queue = VecDeque::from(vec![0.25f32, -0.5, 0.75]);
        }
        // piHPSDR-style chrono: header only, length 8 total values, 2 channels -> 4 frames.
        let chrono = header(0, 48000, FORMAT_FLOAT32, 8, STREAM_TX_CHRONO, 2);
        let reply = handle_binary(&s, &chrono).expect("must answer TX_CHRONO while transmitting");
        assert_eq!(reply.len(), HEADER_LEN + 8 * 4);
        assert_eq!(header_u32(&reply, 5), 8);
        assert_eq!(header_u32(&reply, 6), STREAM_TX_AUDIO);
        assert_eq!(header_u32(&reply, 7), 2);
        assert_eq!(header_u32(&reply, 2), FORMAT_FLOAT32);
        let vals: Vec<f32> =
            reply[HEADER_LEN..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(vals, vec![0.25, 0.25, -0.5, -0.5, 0.75, 0.75, 0.0, 0.0]);
        let tx = s.tx.lock().unwrap();
        assert!(tx.queue.is_empty());
        assert_eq!(tx.chrono_count, 1);
    }

    #[test]
    fn tx_chrono_ignored_when_not_transmitting() {
        let s = shared();
        let chrono = header(0, 48000, FORMAT_FLOAT32, 2048, STREAM_TX_CHRONO, 2);
        assert!(handle_binary(&s, &chrono).is_none());
    }
}
