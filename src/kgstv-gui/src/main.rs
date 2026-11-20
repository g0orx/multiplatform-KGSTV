//! KG-STV desktop interface. Two modes:
//! - **RX**: opens a WAV recording (real off-air or a controlled test recording) or listens live
//!   from the default audio input device (microphone/soundcard), decodes in the background with
//!   `kgstv-modem`'s MSK demodulator + `kgstv-core`'s frame/JPEG decoder, and draws the
//!   image live, tile by tile, similar to KG-STV's original behaviour.
//! - **TX**: takes an image file + optional callsign/text, scales it to 320×240, encodes it into
//!   300 16×16 JPEG tiles, and converts it with `kgstv-core::frame` + `kgstv-modem::session`
//!   into a full KG-STV MSK session (audio); this can be saved as a WAV or
//!   played directly through the speaker.
//!
//! It also shows an eye diagram for visually assessing signal quality/timing.
//! Single code base — builds as a native window on Windows/Linux/macOS
//! (egui/eframe).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use image::imageops::FilterType;
use kgstv_core::constants::Command;
use kgstv_core::InfoChunk;
use kgstv_modem::{FrameResult, LiveCapture};

const TILE: usize = 16;
const IMG_W: usize = 320;
const IMG_H: usize = 240;
const TILES_X: usize = IMG_W / TILE;
const TILES_Y: usize = IMG_H / TILE;
const MAX_LOG_LINES: usize = 500;
/// Fixed sample rate for TX audio generation (for WAV saving and playback).
const TX_SAMPLE_RATE: f64 = 44100.0;

/// EXACTLY the same as the "Scale" drop-down list in the original Windows KGSTV sender (reported
/// by the user) — 16 values; the INDEX (0-15) into the array is encoded in the header's 4-bit `sc`
/// field; the decimal labels (0.07 … 2.0) are purely cosmetic, the human-readable counterpart of
/// the `ImageQ` setting in `kgstv.ini` (see docs/protocol_spec.md, "ImageQ=10 ... sc=10 match
/// exactly"). Default index 10 ("1.0") — the same as the default `ImageQ=10` value
/// cross-checked against a real recording.
const TX_SCALE_LABELS: [&str; 16] =
    ["0.07", "0.08", "0.09", "0.1", "0.2", "0.3", "0.4", "0.5", "0.6", "0.8", "1.0", "1.20", "1.40", "1.60", "1.80", "2.0"];

/// Maximum audio duration kept in the buffer in live mode — anything older is dropped automatically.
///
/// This had been left at 60 s, but that is SHORTER than the duration of a full 300-tile (20×15)
/// image transfer — real off-air recordings last between 78 and 148 s (see docs/protocol_spec.md,
/// "148-second recording"; and the samples/*.wav test patterns: MSK+convolution worst case
/// ~109 s). When the buffer filled and `maybe_kick_live_decode` did a full reset, the image was
/// wiped while only HALF received — user feedback: "it decodes the callsign but the picture
/// doesn't come" (the callsign, being a single short frame right at the start of the transfer,
/// could be caught before 60 s ran out, but the buffer filled and reset before the image ever
/// finished). Increased to 200 s to leave a comfortable margin over the longest real recordings.
const LIVE_BUFFER_SECONDS: f64 = 200.0;
/// How often (in seconds) to attempt a re-decode in live mode.
const LIVE_REDECODE_INTERVAL: Duration = Duration::from_millis(2000);
/// How often (in seconds) to attempt a re-decode in the real-time WAV playback simulation
/// (more often than the live microphone — since the recording is known in advance, for a livelier viewing experience).
const WAV_SIM_REDECODE_INTERVAL: Duration = Duration::from_millis(700);
/// Eye-diagram recomputation interval (not every UI frame).
const EYE_UPDATE_INTERVAL: Duration = Duration::from_millis(300);
/// Audio window used for the eye diagram (last N seconds). The phase folding relies on a SINGLE
/// `anchor` (found near the start of the file) and the NOMINAL symbol duration (`sr/1200`, assumed
/// constant throughout the recording) — in real off-air recordings the (small but real) difference
/// between the sample clocks of the transmitter and the recorder causes a phase drift that
/// accumulates the longer the window is (clean/synthetic OH2UDS test signals do not have this
/// problem — verified via user feedback: "the sounds I recorded off the air ... look doubled up
/// on top of each other"). Keeping the window short keeps this accumulation limited; a full fix
/// would need local/adaptive clock recovery (drift compensation) — left out of scope.
const EYE_WINDOW_SECONDS: f64 = 0.4;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    Rx,
    Tx,
}

enum DecodeMsg {
    /// Sent as soon as the audio is loaded (before decoding starts) — carries the raw samples to
    /// the UI thread so the eye diagram also works in file mode.
    AudioReady { sample_rate: f64, samples: Arc<Vec<f64>> },
    Started { label: String, sample_rate: f64, n_samples: usize },
    Frame(FrameResult),
    /// `frames_this_pass`: total number of frames found by THIS pass (from the start, before
    /// `skip_frames`) — for deciding whether or not to cache the anchor (see the `DecodeMsg::Done`
    /// handler: if 0, the anchor is not a real sync lock and must NOT be cached).
    Done { anchor: f64, frames_this_pass: usize },
    Error(String),
}

enum TxMsg {
    Progress { done: usize, total: usize },
    /// `frame_bounds`: `(com, x, y, cumulative_sample_count_so_far)` for each frame —
    /// for computing "which tiles have been sent so far" during playback (see
    /// `tx_sent_up_to_elapsed`).
    Done { sample_rate: f64, samples: Arc<Vec<f64>>, frame_bounds: Arc<Vec<(u8, u8, u8, usize)>> },
    PlaybackDone,
    PlaybackError(String),
}

/// The settings an encoding was made with. If any of them changes afterwards, the encoded TX
/// audio no longer matches what the user sees and is discarded (see `discard_stale_tx_audio`).
#[derive(Clone, PartialEq)]
struct TxParams {
    callsign: String,
    message: String,
    quality: u8,
    conv: bool,
    fsk4: bool,
}

/// Key under which `PersistedSettings` is stored by eframe's persistence (saved on exit and
/// periodically, in the platform's app-data directory).
const SETTINGS_KEY: &str = "kgstv_settings";

/// User settings remembered between restarts: TX options, callsign/message, TCI options, and the
/// audio devices (by unique cpal device ID, `None` = system default) and level sliders.
/// `#[serde(default)]` keeps older/newer settings files loadable — missing fields fall back to
/// the `Default` values (which mirror `KgstvApp::default()`).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct PersistedSettings {
    tx_callsign: String,
    tx_message: String,
    tx_quality: u8,
    tx_conv: bool,
    tx_fsk4: bool,
    tci_host: String,
    tci_port: u16,
    tci_rx_audio: bool,
    tci_tx_audio: bool,
    tci_freq_khz: String,
    tci_mode: String,
    input_device: Option<String>,
    output_device: Option<String>,
    input_gain: f32,
    output_gain: f32,
}

impl Default for PersistedSettings {
    /// Must mirror the corresponding initial values in `KgstvApp::default()`.
    fn default() -> Self {
        PersistedSettings {
            tx_callsign: String::new(),
            tx_message: String::new(),
            tx_quality: 10,
            tx_conv: false,
            tx_fsk4: false,
            tci_host: "127.0.0.1".to_string(),
            tci_port: kgstv_modem::TCI_DEFAULT_PORT,
            tci_rx_audio: false,
            tci_tx_audio: false,
            tci_freq_khz: "14233.000".to_string(),
            tci_mode: "usb".to_string(),
            input_device: None,
            output_device: None,
            input_gain: 1.0,
            output_gain: 1.0,
        }
    }
}

struct KgstvApp {
    image_rgba: Vec<u8>,
    texture: Option<egui::TextureHandle>,
    texture_dirty: bool,
    log_lines: Vec<String>,
    status: String,
    rx: Option<Receiver<DecodeMsg>>,
    decoding: bool,
    tiles_ok: usize,
    tiles_total: usize,
    frames_total: usize,
    current_file: Option<String>,
    /// Most recent CRC-verified callsign/text — since log lines show only frame metadata
    /// (not the raw text), these are shown separately/prominently in the log panel.
    last_callsign: Option<String>,
    last_text: Option<String>,

    // Audio input/output device selection + level control (shared by RX and TX).
    input_devices: Vec<kgstv_modem::AudioDeviceInfo>,
    output_devices: Vec<kgstv_modem::AudioDeviceInfo>,
    /// `None` = system default.
    selected_input_device: Option<String>,
    selected_output_device: Option<String>,
    /// Multiplier applied to input/output samples (1.0 = no change). The input gain is written to
    /// `LiveCapture::set_gain` every frame during live capture (takes effect instantly without
    /// restarting the stream); the output gain is applied at playback time.
    input_gain: f32,
    output_gain: f32,

    // Radio control + audio over TCI (ExpertSDR / piHPSDR / deskHPSDR ...).
    tci: Option<kgstv_modem::TciClient>,
    tci_host: String,
    tci_port: u16,
    /// Live decode takes its audio from the radio's TCI RX stream instead of the sound card.
    tci_rx_audio: bool,
    /// "Send Image"/"Send Text" transmits through the radio over TCI (keys PTT) instead of the sound card.
    tci_tx_audio: bool,
    /// Frequency entry box (kHz) and mode selection for tuning the radio.
    tci_freq_khz: String,
    tci_mode: String,

    // Live decode.
    live: Option<LiveCapture>,
    last_live_decode: Instant,
    /// Number of frames applied to the UI so far — same purpose as `wav_sim_frames_applied`
    /// (see `maybe_kick_live_decode`).
    live_frames_applied: usize,
    /// Sync anchor found in the first pass — used directly in later passes without SEARCHING
    /// again (see the `maybe_kick_live_decode` documentation — since the search range changes from
    /// pass to pass, re-searching could find a different anchor and break the whole walk).
    ///
    /// Stored as an **ABSOLUTE** sample position (counted from the start of the session) — once the
    /// `LiveCapture` ring buffer starts trimming (`front_absolute_index()`), a LOCAL anchor computed
    /// relative to a previous `snapshot()` points to the WRONG position. On each pass it is
    /// re-localized relative to the current `snapshot()` by subtracting the current `front_absolute_index()`
    /// (see `maybe_kick_live_decode`) — so the anchor stays VALID even when the buffer is trimmed, and
    /// there is no need (as there used to be) to wipe and rescan the WHOLE screen on every trim
    /// (user feedback: "it works fine the first time but then ... it tried to receive all the
    /// frames again").
    live_cached_anchor: Option<f64>,
    /// The `front_absolute_index()` that was current when the `snapshot()` of the pass currently BEING
    /// PROCESSED (or most recently kicked) was taken — needed to convert the returned anchor (in that
    /// pass's LOCAL coordinates) to ABSOLUTE when that pass's `DecodeMsg::Done` arrives.
    live_pass_front_index: u64,

    // Feeding a WAV file "as if playing it" at real-time speed (instead of an instant batch decode
    // — so the tiles arrive at real KG-STV speed and the eye diagram shows the audio received
    // so far).
    wav_sim: Option<kgstv_modem::PlaybackSim>,
    last_wav_sim_decode: Instant,
    /// Number of frames applied to the UI so far (added to the log/tile drawn) — determines which
    /// frames the next pass silently skips so it applies ONLY the new ones
    /// (see `maybe_kick_wav_sim_decode`).
    wav_sim_frames_applied: usize,
    /// Same purpose as `live_cached_anchor`, for the real-time WAV playback simulation.
    wav_sim_cached_anchor: Option<f64>,
    /// `true` after the final decode pass (covering all the data with no safety margin) done once
    /// playback has completely finished — stops the re-decode attempts every 700ms.
    wav_sim_final_pass_done: bool,

    // Eye diagram: the most recently loaded/captured audio (from a file or the live buffer, or the
    // audio generated in TX), periodically redrawn regardless of whether it has been decoded.
    // `last_anchor`: the sync position found by the last decode — ESSENTIAL for aligning the eye
    // diagram's phase to the real symbol grid (see the `recompute_eye_diagram` documentation;
    // folding without an anchor landed on a random/meaningless phase and produced a flat/uninformative plot).
    last_audio: Option<(f64, Arc<Vec<f64>>)>,
    last_anchor: Option<f64>,
    eye_points: Vec<(f32, f32)>,
    last_eye_update: Instant,

    // TX (transmit).
    mode: Mode,
    tx_image_rgba: Vec<u8>,
    tx_texture: Option<egui::TextureHandle>,
    tx_texture_dirty: bool,
    tx_source_name: Option<String>,
    tx_callsign: String,
    tx_message: String,
    tx_quality: u8,
    tx_conv: bool,
    tx_fsk4: bool,
    tx_encoding: bool,
    tx_progress: (usize, usize),
    tx_status: String,
    tx_audio: Option<(f64, Arc<Vec<f64>>)>,
    tx_playing: bool,
    /// For cutting off the currently playing audio early — see `play_tx`/`stop_tx_playback` (user
    /// request: "a button that lets me stop the play step while sending").
    tx_play_stop: Option<Arc<AtomicBool>>,
    /// Cumulative end sample position of each frame (see `TxMsg::Done`) — for computing which
    /// tiles have been sent during playback (`tx_sent_up_to_elapsed`).
    tx_frame_bounds: Arc<Vec<(u8, u8, u8, usize)>>,
    /// The moment playback started — used to compute elapsed time for the progress bar and tile
    /// marking (instead of sample-precise tracking — since `play_blocking` may resample for the
    /// device, the sample count may not match time exactly, but the duration does).
    tx_play_started: Option<Instant>,
    /// "📨 Send Text" / "📤 Send Image" buttons: automatically start playback as well once encoding finishes (user
    /// request: "when I press the button it should both encode and play").
    tx_auto_play_after_encode: bool,
    /// Whether the encode currently running includes the 300 image tiles (`start_tx_encode`'s
    /// `include_image`), and whether the finished `tx_audio` does — "📤 Send Image" must not
    /// replay text-only audio left over from "📨 Send Text", so it re-encodes with the image first.
    tx_encoding_includes_image: bool,
    tx_audio_includes_image: bool,
    /// Settings of the encode currently running, and of the finished `tx_audio`.
    tx_encoding_params: Option<TxParams>,
    tx_audio_params: Option<TxParams>,
    tx_rx: Option<Receiver<TxMsg>>,
}

impl Default for KgstvApp {
    fn default() -> Self {
        // Initial canvas: a flat dark grey "not received yet" background, like the KG-STV screen.
        let mut image_rgba = vec![0u8; IMG_W * IMG_H * 4];
        for px in image_rgba.chunks_exact_mut(4) {
            px[0] = 40;
            px[1] = 40;
            px[2] = 40;
            px[3] = 255;
        }
        KgstvApp {
            image_rgba,
            texture: None,
            texture_dirty: true,
            log_lines: Vec::new(),
            status: "Ready — open a WAV recording or start live decoding.".to_string(),
            rx: None,
            decoding: false,
            tiles_ok: 0,
            tiles_total: 0,
            frames_total: 0,
            current_file: None,
            last_callsign: None,
            last_text: None,
            input_devices: kgstv_modem::list_input_devices(),
            output_devices: kgstv_modem::list_output_devices(),
            selected_input_device: None,
            selected_output_device: None,
            input_gain: 1.0,
            output_gain: 1.0,
            tci: None,
            tci_host: "127.0.0.1".to_string(),
            tci_port: kgstv_modem::TCI_DEFAULT_PORT,
            tci_rx_audio: false,
            tci_tx_audio: false,
            tci_freq_khz: "14233.000".to_string(),
            tci_mode: "usb".to_string(),
            live: None,
            last_live_decode: Instant::now(),
            live_frames_applied: 0,
            live_cached_anchor: None,
            live_pass_front_index: 0,
            wav_sim: None,
            last_wav_sim_decode: Instant::now(),
            wav_sim_frames_applied: 0,
            wav_sim_cached_anchor: None,
            wav_sim_final_pass_done: false,
            last_audio: None,
            last_anchor: None,
            eye_points: Vec::new(),
            last_eye_update: Instant::now(),

            mode: Mode::Rx,
            tx_image_rgba: {
                let mut buf = vec![0u8; IMG_W * IMG_H * 4];
                for px in buf.chunks_exact_mut(4) {
                    px[0] = 60;
                    px[1] = 60;
                    px[2] = 60;
                    px[3] = 255;
                }
                buf
            },
            tx_texture: None,
            tx_texture_dirty: true,
            tx_source_name: None,
            tx_callsign: String::new(),
            tx_message: String::new(),
            tx_quality: 10,
            tx_conv: false,
            tx_fsk4: false,
            tx_encoding: false,
            tx_progress: (0, 0),
            tx_status: "Choose an image.".to_string(),
            tx_audio: None,
            tx_playing: false,
            tx_play_stop: None,
            tx_frame_bounds: Arc::new(Vec::new()),
            tx_play_started: None,
            tx_auto_play_after_encode: false,
            tx_encoding_includes_image: false,
            tx_audio_includes_image: false,
            tx_encoding_params: None,
            tx_audio_params: None,
            tx_rx: None,
        }
    }
}

impl KgstvApp {
    fn reset_decode_state(&mut self) {
        self.log_lines.clear();
        self.tiles_ok = 0;
        self.tiles_total = 0;
        self.frames_total = 0;
        self.last_callsign = None;
        self.last_text = None;
        for px in self.image_rgba.chunks_exact_mut(4) {
            px[0] = 40;
            px[1] = 40;
            px[2] = 40;
            px[3] = 255;
        }
        self.texture_dirty = true;
    }

    fn open_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        let path_str = path.display().to_string();
        self.current_file = Some(path_str.clone());
        self.status = format!("Loading: {path_str}");
        self.reset_decode_state();
        self.decoding = true;
        self.live = None; // file mode stops live capture
        self.wav_sim = None; // and the real-time playback simulation
        self.last_anchor = None; // the old anchor is no longer valid for a new file

        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);

        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            let audio = match kgstv_modem::load_mono(&path) {
                Ok(a) => a,
                Err(e) => {
                    let _ = tx.send(DecodeMsg::Error(format!("Could not read WAV: {e}")));
                    ctx1.request_repaint();
                    return;
                }
            };
            let samples = Arc::new(audio.samples);
            let _ = tx.send(DecodeMsg::AudioReady { sample_rate: audio.sample_rate, samples: samples.clone() });
            run_decode(&ctx1, &tx, path_str, audio.sample_rate, samples, 0, None);
        });
    }

    /// "📻 Radio (TCI)" section, shown in both the RX and TX panels: connect to a TCI server,
    /// show the radio's frequency/mode/TX state, tune it, and choose TCI as the RX/TX audio path.
    fn tci_panel(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("📻 Radio (TCI)", |ui| {
            let status = self.tci.as_ref().map(|c| c.status());
            let connected = status.as_ref().is_some_and(|s| s.state == kgstv_modem::TciState::Connected);
            let connecting = status.as_ref().is_some_and(|s| s.state == kgstv_modem::TciState::Connecting);
            ui.horizontal(|ui| {
                ui.label("Host");
                ui.add_enabled(
                    self.tci.is_none(),
                    egui::TextEdit::singleline(&mut self.tci_host).desired_width(110.0),
                );
                ui.label("Port");
                ui.add_enabled(self.tci.is_none(), egui::DragValue::new(&mut self.tci_port).range(1..=65535));
                if self.tci.is_none() {
                    if ui.button("Connect").clicked() {
                        self.tci = Some(kgstv_modem::TciClient::connect(&self.tci_host, self.tci_port));
                    }
                } else if ui.button(if connecting { "Cancel" } else { "Disconnect" }).clicked() {
                    self.disconnect_tci();
                }
            });
            match status.as_ref().map(|s| &s.state) {
                None => {
                    ui.label("Not connected.");
                }
                Some(kgstv_modem::TciState::Connecting) => {
                    ui.label("Connecting...");
                }
                Some(kgstv_modem::TciState::Disconnected(reason)) => {
                    let reason = if reason.is_empty() { "Disconnected." } else { reason.as_str() };
                    ui.colored_label(egui::Color32::LIGHT_RED, reason);
                    if ui.button("Clear").clicked() {
                        self.disconnect_tci();
                    }
                }
                Some(kgstv_modem::TciState::Connected) => {
                    let st = status.as_ref().unwrap();
                    ui.label(format!(
                        "Connected: {}{}",
                        st.device.as_deref().unwrap_or("radio"),
                        st.protocol.as_deref().map(|p| format!(" ({p})")).unwrap_or_default()
                    ));
                    let freq = st
                        .frequency_hz
                        .map(|f| format!("{:.3} kHz", f as f64 / 1000.0))
                        .unwrap_or_else(|| "? kHz".to_string());
                    let mode = st.modulation.as_deref().unwrap_or("?").to_uppercase();
                    let txt = egui::RichText::new(format!("{freq}   {mode}")).strong();
                    ui.horizontal(|ui| {
                        ui.label(txt);
                        if st.transmitting {
                            ui.colored_label(egui::Color32::RED, egui::RichText::new("● TX").strong());
                        }
                    });
                }
            }
            ui.add_enabled_ui(connected, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.tci_freq_khz).desired_width(90.0));
                    ui.label("kHz");
                    if ui.button("Tune").clicked() {
                        match self.tci_freq_khz.trim().replace(',', ".").parse::<f64>() {
                            Ok(khz) if khz > 0.0 => {
                                if let Some(c) = &self.tci {
                                    c.set_frequency((khz * 1000.0).round() as u64);
                                }
                            }
                            _ => self.status = format!("Invalid frequency: {}", self.tci_freq_khz),
                        }
                    }
                    egui::ComboBox::from_id_salt("tci_mode")
                        .selected_text(self.tci_mode.to_uppercase())
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for m in ["usb", "digu", "lsb", "digl"] {
                                ui.selectable_value(&mut self.tci_mode, m.to_string(), m.to_uppercase());
                            }
                        });
                    if ui.button("Set mode").clicked() {
                        if let Some(c) = &self.tci {
                            c.set_modulation(&self.tci_mode);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("SSTV:");
                    for khz in [3733.0, 7173.0, 14233.0, 21340.0, 28680.0] {
                        if ui.small_button(format!("{khz:.0}")).clicked() {
                            self.tci_freq_khz = format!("{khz:.3}");
                            if let Some(c) = &self.tci {
                                c.set_frequency((khz * 1000.0) as u64);
                            }
                        }
                    }
                });
            });
            // Switching the RX audio source while live decoding is running restarts the capture on
            // the new source (the old buffer/anchor belong to the old source, so decode state resets).
            let rx_toggled = ui
                .checkbox(&mut self.tci_rx_audio, "Receive audio from the radio (TCI)")
                .on_hover_text("Live decode takes its audio from the radio over TCI instead of the sound card")
                .changed();
            if rx_toggled && self.live.is_some() {
                self.live = None;
                self.toggle_live(ui.ctx());
            }
            if self.tci_rx_audio && !connected {
                ui.weak("  (connect to the radio first)");
            }
            ui.add_enabled(
                !self.tx_playing,
                egui::Checkbox::new(&mut self.tci_tx_audio, "Transmit through the radio (TCI, keys PTT)"),
            )
            .on_disabled_hover_text("Can't change the transmit path while transmitting");
        });
    }

    fn disconnect_tci(&mut self) {
        if let Some(c) = self.tci.take() {
            c.disconnect();
        }
        if self.tci_rx_audio && self.live.is_some() {
            self.live = None;
            self.status = "Live decoding stopped (TCI disconnected).".to_string();
        }
    }

    fn toggle_live(&mut self, ctx: &egui::Context) {
        if self.live.is_some() {
            self.live = None;
            self.status = "Live decoding stopped.".to_string();
            return;
        }
        let started = if self.tci_rx_audio {
            match &self.tci {
                Some(c) => c.start_rx_audio(LIVE_BUFFER_SECONDS, self.input_gain),
                None => Err("TCI audio selected but not connected to a radio".to_string()),
            }
        } else {
            LiveCapture::start(LIVE_BUFFER_SECONDS, self.selected_input_device.as_deref(), self.input_gain)
        };
        match started {
            Ok(cap) => {
                let source = if self.tci_rx_audio { "radio (TCI)" } else { "sound card" };
                self.status = format!("Listening live from {source} @ {:.0} Hz — waiting...", cap.sample_rate);
                self.current_file =
                    Some(if self.tci_rx_audio { "(TCI radio audio)" } else { "(live input)" }.to_string());
                self.reset_decode_state();
                self.wav_sim = None; // live mode stops real-time WAV playback
                self.live = Some(cap);
                self.live_frames_applied = 0;
                self.live_cached_anchor = None;
                self.live_pass_front_index = 0;
                self.last_live_decode = Instant::now() - LIVE_REDECODE_INTERVAL; // trigger immediately
                let _ = ctx;
            }
            Err(e) => {
                self.status = format!("Could not start live capture: {e}");
            }
        }
    }

    /// Starts feeding a WAV file "as if playing it" over a time equal to its real playback duration,
    /// instead of an instant batch decode (see `kgstv_modem::PlaybackSim`) — so the tiles appear on
    /// screen at real KG-STV speed and the eye diagram can show only the audio received
    /// so far.
    fn start_wav_sim(&mut self, path: PathBuf) {
        match kgstv_modem::load_mono(&path) {
            Ok(audio) => {
                let name = path.display().to_string();
                self.status = format!(
                    "Playing in real time: {name} ({:.1} s)",
                    audio.samples.len() as f64 / audio.sample_rate
                );
                self.current_file = Some(name);
                self.reset_decode_state();
                self.live = None; // real-time playback stops live microphone listening
                self.last_anchor = None;
                self.wav_sim = Some(kgstv_modem::PlaybackSim::start(audio.samples, audio.sample_rate));
                self.wav_sim_frames_applied = 0;
                self.wav_sim_cached_anchor = None;
                self.wav_sim_final_pass_done = false;
                self.last_wav_sim_decode = Instant::now() - WAV_SIM_REDECODE_INTERVAL; // trigger immediately
            }
            Err(e) => {
                self.status = format!("Could not read WAV: {e}");
            }
        }
    }

    /// In live mode, periodically tries to re-decode the accumulated buffer (if the previous decode
    /// has finished). Since `decode_wav_sequential_cb` has no stateful API, every pass MUST
    /// scan the buffer from the start, but, as in `maybe_kick_wav_sim_decode`, we do this without
    /// showing the UI a "starting from scratch" (without wiping the image/log, only applying NEW
    /// frames not seen in previous passes) — otherwise it gave the impression that every ~2 s the
    /// screen went blank/the frame counter returned to 0 (user feedback).
    ///
    /// **Buffer trimming:** `LiveCapture`'s ring buffer starts dropping old samples once
    /// `LIVE_BUFFER_SECONDS` is full (see `LiveCapture::front_absolute_index`) — this is a STATE that
    /// stays true on EVERY pass after that (after 200 s the buffer is ALWAYS "full"), NOT a
    /// one-off event. Previously this was detected by a "is the buffer full" (`raw.len() >= capacity`)
    /// check, and every time `reset_decode_state()` wiped the WHOLE screen (image/log/
    /// anchor) and rescanned from the start — in a live listen lasting over 200 s this led to a
    /// "re-receiving everything" appearance repeating FOREVER every ~2 seconds
    /// (user feedback: "it works fine the first time but then ... it tried to receive all
    /// the frames again"). Fix: the cached anchor is now kept as an ABSOLUTE position
    /// (see the `live_cached_anchor` documentation) — on each pass it is RE-localized relative to the
    /// current `snapshot()` by subtracting the current `front_absolute_index()`.
    /// If the audio the anchor points to has REALLY been dropped (local position comes out negative), ONLY
    /// the anchor cache is cleared (a fresh search is triggered) — the screen/image/log is NEVER wiped.
    fn maybe_kick_live_decode(&mut self, ctx: &egui::Context) {
        let Some(live) = &self.live else { return };
        if self.decoding || self.last_live_decode.elapsed() < LIVE_REDECODE_INTERVAL {
            return;
        }
        let sample_rate = live.sample_rate;
        let front_index = live.front_absolute_index();
        let raw = live.snapshot();
        // Safety margin: same rationale as in `maybe_kick_wav_sim_decode` — since in incremental
        // application no frame is revisited, this avoids the risk of permanently drawing a frame
        // that is cut off at the very end of the buffer incorrectly.
        let margin = ((0.25 * sample_rate) as usize).min(raw.len());
        let safe_len = raw.len() - margin;
        if safe_len < 4096 {
            return; // not enough "safe" samples yet for a meaningful decode attempt
        }

        // Convert the cached (ABSOLUTE) anchor into a LOCAL position relative to this pass's snapshot.
        // If the audio the anchor points to has really been dropped from the buffer (local position < 0),
        // it can no longer be used — ONLY clear the cache (DO NOT WIPE the screen: frames already
        // decoded remain valid; the next pass does a fresh search and finds a NEW
        // session if there is one).
        let local_known_anchor = self.live_cached_anchor.and_then(|abs| {
            let local = abs - front_index as f64;
            if local >= 0.0 { Some(local) } else { None }
        });
        if self.live_cached_anchor.is_some() && local_known_anchor.is_none() {
            self.live_cached_anchor = None;
            self.live_frames_applied = 0; // the frame count relative to the old anchor is now meaningless
        }

        self.last_live_decode = Instant::now();
        self.live_pass_front_index = front_index;
        let samples = Arc::new(raw[..safe_len].to_vec());
        self.last_audio = Some((sample_rate, samples.clone()));

        self.decoding = true;
        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);
        let ctx1 = ctx.clone();
        let skip = self.live_frames_applied;
        let known_anchor = local_known_anchor;
        std::thread::spawn(move || {
            run_decode(&ctx1, &tx, "(live)".to_string(), sample_rate, samples, skip, known_anchor);
        });
    }

    /// Similar to `maybe_kick_live_decode`, but with TWO important differences — both to solve the
    /// "it keeps going back to the start and rescanning" problem the user saw:
    ///
    /// 1. **State is not reset, it is applied incrementally**: since `decode_wav_sequential_cb` has
    ///    no API that remembers state, every pass MUST walk from the start, but instead of
    ///    showing this to the UI as a "clean page every time" via `reset_decode_state()`, we
    ///    make `run_decode` silently skip frames already seen in previous passes (`wav_sim_frames_applied`)
    ///    and apply ONLY the new frames — the image/log is never wiped, it only
    ///    grows.
    /// 2. **Safety margin**: decoding right up to the very end of the buffer risks wrongly decoding a
    ///    frame cut off right in the middle at that moment (with corrupted bits) and PERMANENTLY drawing
    ///    it on screen (since it is no longer wiped on every pass, unlike the previous behaviour this would
    ///    never be corrected) — so we leave the last ~0.25 s out of the decode.
    fn maybe_kick_wav_sim_decode(&mut self, ctx: &egui::Context) {
        let Some(sim) = &self.wav_sim else { return };
        if self.wav_sim_final_pass_done {
            return; // playback finished and the final (full) pass is already done — no need to try again
        }
        if self.decoding || self.last_wav_sim_decode.elapsed() < WAV_SIM_REDECODE_INTERVAL {
            return;
        }
        let sample_rate = sim.sample_rate;
        let raw = sim.snapshot();
        let is_done = sim.is_done();
        // If playback has finished there is no longer a "moving target" — we can decode
        // everything remaining without needing a safety margin.
        let safety_margin = if is_done { 0 } else { ((0.25 * sample_rate) as usize).min(raw.len()) };
        let safe_len = raw.len() - safety_margin;
        if safe_len < 4096 {
            return; // not enough "safe" data yet for a meaningful decode attempt
        }
        self.last_wav_sim_decode = Instant::now();
        self.wav_sim_final_pass_done = is_done;
        let samples = Arc::new(raw[..safe_len].to_vec());
        let received_secs = samples.len() as f64 / sample_rate;
        let total_secs = sim.total_len() as f64 / sample_rate;
        self.last_audio = Some((sample_rate, samples.clone()));

        self.decoding = true;
        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);
        let ctx1 = ctx.clone();
        let label = format!("(real time, {received_secs:.1}/{total_secs:.1} s)");
        let skip = self.wav_sim_frames_applied;
        // `PlaybackSim`'s buffer (unlike the live microphone's) never drops old data, it only
        // grows — the anchor found stays valid throughout the session.
        let known_anchor = self.wav_sim_cached_anchor;
        std::thread::spawn(move || {
            run_decode(&ctx1, &tx, label, sample_rate, samples, skip, known_anchor);
        });
    }

    /// Recomputes the eye diagram. **Critical:** the phase folding must be aligned to the real sync
    /// `anchor` — folding from an arbitrary start such as `(t/tb)%1` without knowing the anchor shifts
    /// the eye diagram's "open at the midpoint" structure to a random phase and produces a flat/meaningless
    /// plot (this was a bug verified with a real recording — see `docs/protocol_spec.md`).
    /// So nothing is drawn until `last_anchor` is known (before decoding has completed).
    ///
    /// **Signal choice:** the matched-filter correlation of `ToneClock::soft_metric_at` (the first
    /// approach tried) produced a soft/sinusoidal curve within a symbol — NOT the expected "flat
    /// plateau + sharp transition" look of a classic oscilloscope eye diagram (verified by user
    /// feedback and a reference image). Instead `kgstv_modem::
    /// instantaneous_freq` (instantaneous frequency discriminator) is used — since the frequency
    /// really stays flat within a symbol (by the nature of CPFSK) and changes only at symbol
    /// boundaries, this produces exactly the classic eye-diagram shape. The frequency is normalized
    /// to [-1,1] by `±300Hz` (MSK's nominal deviation) — 4L-FSK's intermediate levels
    /// (1400/1600Hz) naturally land at ±0.33.
    fn recompute_eye_diagram(&mut self) {
        if self.last_eye_update.elapsed() < EYE_UPDATE_INTERVAL {
            return;
        }
        self.last_eye_update = Instant::now();
        let (Some((sample_rate, samples)), Some(anchor)) = (&self.last_audio, self.last_anchor) else {
            self.eye_points.clear();
            return;
        };
        let sr = *sample_rate;
        let n = samples.len();
        let take = ((EYE_WINDOW_SECONDS * sr) as usize).min(n);
        if take < 256 {
            self.eye_points.clear();
            return;
        }
        let start = n - take;
        let filtered = kgstv_modem::bandpass_filtfilt(&samples[start..], sr, 900.0, 2200.0);
        let tb = sr / kgstv_modem::BAUD;
        if tb < 1.0 {
            self.eye_points.clear();
            return;
        }
        const CENTER_FREQ: f64 = 1500.0;
        const LP_CUTOFF: f64 = 500.0;
        const DEVIATION: f64 = 300.0; // MSK's nominal deviation — normalization scale
        let freq = kgstv_modem::instantaneous_freq(&filtered, sr, CENTER_FREQ, LP_CUTOFF);

        let mut points = Vec::with_capacity(freq.len());
        for (i, &f) in freq.iter().enumerate() {
            let m = (((f - CENTER_FREQ) / DEVIATION) as f32).clamp(-1.3, 1.3);
            let abs_t = start as f64 + i as f64;
            let phase = ((abs_t - anchor) / tb).rem_euclid(1.0) as f32;
            points.push((phase, m));
        }

        // Automatic centring: at `anchor` there may be a small systematic offset that is irrelevant
        // to MSK's own (incoherent) bit decision but can shift the eye diagram's phase
        // (user feedback: "the eye diagram ... is not exactly centred, it's off to the right and left").
        // By finding the eye's most OPEN point (where the frequency is most stable/has the lowest variance)
        // and aligning it to phase=0.5, the diagram always looks centred regardless of small errors
        // in the anchor.
        if !points.is_empty() {
            const N_BINS: usize = 24;
            let mut sum = [0.0f64; N_BINS];
            let mut sum_sq = [0.0f64; N_BINS];
            let mut count = [0usize; N_BINS];
            for &(phase, m) in &points {
                let idx = ((phase * N_BINS as f32) as usize).min(N_BINS - 1);
                sum[idx] += m as f64;
                sum_sq[idx] += (m as f64) * (m as f64);
                count[idx] += 1;
            }
            let mut best_bin = None;
            let mut best_var = f64::INFINITY;
            for i in 0..N_BINS {
                if count[i] < 4 {
                    continue;
                }
                let mean = sum[i] / count[i] as f64;
                let var = sum_sq[i] / count[i] as f64 - mean * mean;
                if var < best_var {
                    best_var = var;
                    best_bin = Some(i);
                }
            }
            if let Some(bin) = best_bin {
                let best_phase = (bin as f32 + 0.5) / N_BINS as f32;
                let shift = 0.5 - best_phase;
                for p in points.iter_mut() {
                    p.0 = (p.0 + shift).rem_euclid(1.0);
                }
            }
        }
        self.eye_points = points;
    }

    fn load_tx_image(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        match image::open(&path) {
            Ok(img) => {
                let resized = img.resize_exact(IMG_W as u32, IMG_H as u32, FilterType::Lanczos3);
                self.tx_image_rgba = resized.to_rgba8().into_raw();
                self.tx_texture_dirty = true;
                self.tx_source_name = Some(name);
                self.tx_audio = None; // audio generated with the old image is no longer valid
                self.tx_audio_includes_image = false;
                self.tx_audio_params = None;
                self.tx_status = "Image loaded — ready to send.".to_string();
            }
            Err(e) => {
                self.tx_status = format!("Could not open image: {e}");
            }
        }
    }

    /// Encodes the 300 tiles (+ optional callsign/text frame) and converts them into MSK audio.
    /// `include_image`: when `false`, the 300 image tiles are not added at all — only the callsign
    /// (if any) + text are sent (user request: "the send next to the text box should send only
    /// the text" — for the "📨 Send Text" shortcut next to the Message box, so it quickly sends only the
    /// text without encoding the image).
    fn start_tx_encode(&mut self, ctx: &egui::Context, include_image: bool) {
        self.tx_encoding = true;
        self.tx_encoding_includes_image = include_image;
        self.tx_encoding_params = Some(self.current_tx_params());
        self.tx_progress = (0, 0);
        self.tx_status = "Encoding...".to_string();
        let rgba = self.tx_image_rgba.clone();
        let callsign = self.tx_callsign.clone();
        let message = self.tx_message.clone();
        let quality = self.tx_quality.min(15); // sc is a 4-bit field: 0-15, the full TX_SCALE_LABELS range
        let conv = self.tx_conv;
        let fsk4 = self.tx_fsk4;
        let modulation = fsk4 as u8;

        let (tx, rx) = mpsc::channel();
        self.tx_rx = Some(rx);
        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            let mut frames: Vec<(InfoChunk, Vec<u8>)> = Vec::new();
            if !callsign.trim().is_empty() {
                let bits = ascii_to_bits(callsign.trim());
                let hdr = InfoChunk {
                    sys: 0,
                    com: Command::Callsign as u8,
                    c: 0,
                    m: 0,
                    x: 0,
                    y: 0,
                    sc: quality,
                    size: bits.len() as u16,
                    crc: 0,
                };
                frames.push((hdr, bits));
            }
            if !message.trim().is_empty() {
                let bits = ascii_to_bits(message.trim());
                let hdr = InfoChunk {
                    sys: 0,
                    com: Command::Text as u8,
                    c: conv as u8,
                    m: modulation,
                    x: 0,
                    y: 0,
                    sc: quality,
                    size: bits.len() as u16,
                    crc: 0,
                };
                frames.push((hdr, bits));
            }
            if include_image {
                for ty in 0..TILES_Y {
                    for tx_ in 0..TILES_X {
                        let rgb = extract_tile_rgb(&rgba, tx_, ty);
                        let tile = kgstv_core::rgb_to_tile(&rgb);
                        let bits = kgstv_core::encode_tile(&tile, quality);
                        let hdr = InfoChunk {
                            sys: 0,
                            com: Command::Image as u8,
                            c: conv as u8,
                            m: modulation,
                            x: tx_ as u8,
                            y: ty as u8,
                            sc: quality,
                            size: bits.len() as u16,
                            crc: 0,
                        };
                        frames.push((hdr, bits));
                    }
                }
            }

            // Collect each frame's cumulative end sample position — for computing "which tiles have
            // been sent so far" during playback (see the `TxMsg::Done` documentation).
            let cum_positions = Arc::new(Mutex::new(Vec::with_capacity(frames.len())));
            let cum_positions_cb = cum_positions.clone();
            let tx2 = tx.clone();
            let samples =
                kgstv_modem::encode_session_audio_cb(&frames, TX_SAMPLE_RATE, move |done, total, cum_len| {
                    let _ = tx2.send(TxMsg::Progress { done, total });
                    cum_positions_cb.lock().unwrap().push(cum_len);
                });
            let cum_positions = cum_positions.lock().unwrap().clone();
            let frame_bounds: Vec<(u8, u8, u8, usize)> = frames
                .iter()
                .zip(cum_positions.iter())
                .map(|((hdr, _), &end)| (hdr.com, hdr.x, hdr.y, end))
                .collect();
            let _ = tx.send(TxMsg::Done {
                sample_rate: TX_SAMPLE_RATE,
                samples: Arc::new(samples),
                frame_bounds: Arc::new(frame_bounds),
            });
            ctx1.request_repaint();
        });
    }

    fn save_tx_wav(&mut self) {
        let Some((sr, samples)) = &self.tx_audio else { return };
        let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).set_file_name("kgstv_tx.wav").save_file()
        else {
            return;
        };
        match kgstv_modem::save_mono(&path, sr.round() as u32, samples) {
            Ok(()) => self.tx_status = format!("Saved: {}", path.display()),
            Err(e) => self.tx_status = format!("Could not save WAV: {e}"),
        }
    }

    fn play_tx(&mut self, ctx: &egui::Context) {
        let Some((sr, samples)) = self.tx_audio.clone() else { return };
        if self.tx_playing {
            return;
        }
        let tci = if self.tci_tx_audio {
            match &self.tci {
                Some(c) if c.is_connected() => Some(c.clone()),
                _ => {
                    self.tx_status = "Transmit via TCI selected but not connected to a radio.".to_string();
                    return;
                }
            }
        } else {
            None
        };
        self.tx_playing = true;
        self.tx_status = if tci.is_some() { "Transmitting via TCI..." } else { "Playing..." }.to_string();
        self.tx_play_started = Some(Instant::now());
        let stop = Arc::new(AtomicBool::new(false));
        self.tx_play_stop = Some(stop.clone());
        let (tx, rx) = mpsc::channel();
        self.tx_rx = Some(rx);
        let ctx1 = ctx.clone();
        let device = self.selected_output_device.clone();
        let gain = self.output_gain;
        std::thread::spawn(move || {
            let result = match &tci {
                Some(c) => c.transmit_blocking(&samples, sr, gain, Some(stop)),
                None => kgstv_modem::play_blocking(&samples, sr, device.as_deref(), gain, Some(stop)),
            };
            let msg = match result {
                Ok(()) => TxMsg::PlaybackDone,
                Err(e) => TxMsg::PlaybackError(e),
            };
            let _ = tx.send(msg);
            ctx1.request_repaint();
        });
    }

    /// Cuts off an ongoing "Send Image"/"Send Text" playback early (see `play_tx`/`tx_play_stop` — user request).
    /// The actual state change (`tx_playing = false`) arrives via `TxMsg::PlaybackDone` in
    /// `drain_tx_messages` when the background thread returns — only the flag is set here.
    fn stop_tx_playback(&mut self) {
        if let Some(stop) = &self.tx_play_stop {
            stop.store(true, Ordering::Relaxed);
            self.tx_status = "Stopping...".to_string();
        }
    }

    /// Based on the TIME elapsed since playback started (instead of sample-precise tracking — since
    /// `play_blocking` may resample when the device requires it, the real sample count may not match
    /// exactly, but the duration does), returns the set of (x,y) tiles SENT so far in the TX
    /// preview — only meaningful WHILE PLAYING, otherwise an empty set (user request:
    /// "let's mark the sent part on the picture").
    fn tx_tiles_sent_so_far(&self) -> HashSet<(u8, u8)> {
        let mut out = HashSet::new();
        if !self.tx_playing {
            return out;
        }
        let (Some(started), Some((sr, _))) = (self.tx_play_started, &self.tx_audio) else {
            return out;
        };
        let elapsed_samples = (started.elapsed().as_secs_f64() * sr) as usize;
        for &(com, x, y, end_sample) in self.tx_frame_bounds.iter() {
            if com == Command::Image as u8 && end_sample <= elapsed_samples {
                out.insert((x, y));
            }
        }
        out
    }

    fn drain_tx_messages(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.tx_rx else { return };
        // "📨 Send Text" button: to automatically start playback once encoding finishes — while `self.tx_rx`
        // is borrowed we cannot directly call `play_tx`, which needs `&mut self` (borrow
        // checker), so we ONLY collect the flag here and apply it below once the loop ends (once
        // the borrow is released).
        let mut should_auto_play = false;
        loop {
            let msg = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => break,
                // Critical fix (user feedback: "it sends but sometimes it
                // doesn't send, when I try again and again I can send"): if the background
                // encode/play thread PANICS (e.g. due to an unexpected audio device error)
                // and dies without sending a final message, the channel becomes "Disconnected" —
                // previously this case was handled the SAME way as `Empty` (silently leaving
                // the loop), i.e. `tx_encoding`/`tx_playing` stayed `true` FOREVER and
                // "Encoding.../Playing..." stayed stuck on screen, with the "📨 Send Text"/"📤 Send Image"
                // buttons permanently disabled (the user, without noticing this,
                // kept retrying, and it seemed "not to send" until the application was
                // probably restarted at some point). Now when the connection drops,
                // the state flags that still look busy are reset IMMEDIATELY and an error message
                // is shown — the user can retry without restarting the interface.
                Err(TryRecvError::Disconnected) => {
                    if self.tx_encoding || self.tx_playing {
                        self.tx_encoding = false;
                        self.tx_playing = false;
                        self.tx_play_started = None;
                        self.tx_play_stop = None;
                        self.tx_auto_play_after_encode = false;
                        self.tx_status =
                            "Error: background task stopped unexpectedly (you can try again).".to_string();
                    }
                    break;
                }
            };
            match msg {
                TxMsg::Progress { done, total } => {
                    self.tx_progress = (done, total);
                    self.tx_status = format!("Encoding... {done}/{total} frames");
                }
                TxMsg::Done { sample_rate, samples, frame_bounds } => {
                    self.tx_encoding = false;
                    self.tx_status = format!(
                        "Ready — {:.1} s of audio ({} samples @ {sample_rate:.0} Hz)",
                        samples.len() as f64 / sample_rate,
                        samples.len()
                    );
                    self.last_audio = Some((sample_rate, samples.clone()));
                    // The TX audio we generate ourselves always starts with the preamble at sample 0
                    // (see encode_session_audio_cb) — anchor=0.0 is exactly right.
                    self.last_anchor = Some(0.0);
                    self.tx_audio = Some((sample_rate, samples));
                    self.tx_audio_includes_image = self.tx_encoding_includes_image;
                    self.tx_audio_params = self.tx_encoding_params.take();
                    self.tx_frame_bounds = frame_bounds;
                    if self.tx_auto_play_after_encode {
                        self.tx_auto_play_after_encode = false;
                        should_auto_play = true;
                    }
                }
                TxMsg::PlaybackDone => {
                    self.tx_playing = false;
                    self.tx_play_started = None;
                    // If the user pressed "Stop", `play_blocking` also returns early with `Ok(())`
                    // (see that function's documentation) — check the flag and show the right message.
                    let stopped_by_user =
                        self.tx_play_stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
                    self.tx_status =
                        if stopped_by_user { "Playback stopped.".to_string() } else { "Playback finished.".to_string() };
                    self.tx_play_stop = None;
                }
                TxMsg::PlaybackError(e) => {
                    self.tx_playing = false;
                    self.tx_play_started = None;
                    self.tx_play_stop = None;
                    self.tx_auto_play_after_encode = false;
                    self.tx_status = format!("Playback error: {e}");
                }
            }
        }
        if should_auto_play {
            self.play_tx(ctx);
        }
    }

    fn drain_messages(&mut self) {
        let Some(rx) = &self.rx else { return };
        loop {
            let msg = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => break,
                // See the documentation of the same fix in `drain_tx_messages` — so that if the decode
                // thread dies without sending a final message (panic etc.), `self.decoding`
                // does not stay `true` forever and show the interface as locked.
                Err(TryRecvError::Disconnected) => {
                    if self.decoding {
                        self.decoding = false;
                        self.status = "Error: decode task stopped unexpectedly (you can try again).".to_string();
                    }
                    break;
                }
            };
            match msg {
                DecodeMsg::AudioReady { sample_rate, samples } => {
                    self.last_audio = Some((sample_rate, samples));
                }
                DecodeMsg::Started { label, sample_rate, n_samples } => {
                    self.status = format!(
                        "{label} decoding: {n_samples} samples @ {sample_rate:.0} Hz ({:.1} s)",
                        n_samples as f64 / sample_rate
                    );
                }
                DecodeMsg::Frame(fr) => {
                    self.frames_total += 1;
                    let hdr = &fr.header;
                    let cmd = Command::from_u8(hdr.com);
                    let modulation = if hdr.m == 1 { "4LFSK" } else { "MSK" };
                    self.log_lines.push(format!(
                        "#{:04} {:?} x={:>2} y={:>2} sc={:>2} size={:>4} {modulation:<5} crc_ok={} viterbi_err={}",
                        self.frames_total, cmd, hdr.x, hdr.y, hdr.sc, hdr.size, fr.data_crc_ok, fr.header_metric
                    ));
                    if self.log_lines.len() > MAX_LOG_LINES {
                        self.log_lines.remove(0);
                    }
                    // The log line shows only the frame metadata (x/y/sc/size) — we additionally decode
                    // the REAL content (raw text) of callsign/text frames and write it both to the log
                    // and to the prominent summary area at the top (otherwise "Some(Callsign)"
                    // was shown and the content never appeared — user
                    // feedback: "it can't decode the callsign").
                    if fr.data_crc_ok {
                        if let Some(data_bits) = &fr.data_bits {
                            let text = bits_to_ascii(data_bits);
                            match cmd {
                                Some(Command::Callsign) => {
                                    self.log_lines.push(format!("      → CALLSIGN: {text}"));
                                    self.last_callsign = Some(text);
                                }
                                Some(Command::Text) => {
                                    self.log_lines.push(format!("      → TEXT: {text}"));
                                    self.last_text = Some(text);
                                }
                                _ => {}
                            }
                        }
                    }
                    if cmd == Some(Command::Image) {
                        self.tiles_total += 1;
                        if let Some(data_bits) = &fr.data_bits {
                            if let Some(tile) = kgstv_core::decode_tile(data_bits, hdr.sc) {
                                let rgb = kgstv_core::tile_to_rgb(&tile);
                                let px0 = hdr.x as usize * TILE;
                                let py0 = hdr.y as usize * TILE;
                                if px0 + TILE <= IMG_W && py0 + TILE <= IMG_H {
                                    for dy in 0..TILE {
                                        for dx in 0..TILE {
                                            let [r, g, b] = rgb[dy][dx];
                                            let idx = ((py0 + dy) * IMG_W + (px0 + dx)) * 4;
                                            self.image_rgba[idx] = r;
                                            self.image_rgba[idx + 1] = g;
                                            self.image_rgba[idx + 2] = b;
                                            self.image_rgba[idx + 3] = 255;
                                        }
                                    }
                                    self.texture_dirty = true;
                                    self.tiles_ok += 1;
                                }
                            }
                        }
                    }
                }
                DecodeMsg::Done { anchor, frames_this_pass } => {
                    self.decoding = false;
                    self.last_anchor = Some(anchor);
                    // Cache the anchor ONLY if this pass actually found at least one frame
                    // (i.e. it was a real sync lock) — otherwise we would cache `find_sync`'s "best
                    // available" (but actually INVALID/noise) result and reuse it forever,
                    // which would mean never being able to sync even when a REAL signal
                    // arrives (this was a bug found via user feedback:
                    // "it didn't decode the callsign ... the image never decoded"). If 0,
                    // the cache is cleared so the next pass does a fresh search.
                    if self.wav_sim.is_some() {
                        // Let the next pass silently skip this many and apply only the new frames
                        // (see `maybe_kick_wav_sim_decode`).
                        self.wav_sim_frames_applied = self.frames_total;
                        self.wav_sim_cached_anchor = if frames_this_pass > 0 { Some(anchor) } else { None };
                    } else if self.live.is_some() {
                        self.live_frames_applied = self.frames_total;
                        // `anchor` is LOCAL relative to this pass's snapshot — in the cache it is kept as an
                        // ABSOLUTE position (see the `live_cached_anchor` documentation), so
                        // it is converted to ABSOLUTE by adding the `live_pass_front_index` that was
                        // current when this pass's snapshot was taken.
                        self.live_cached_anchor =
                            if frames_this_pass > 0 { Some(anchor + self.live_pass_front_index as f64) } else { None };
                    }
                    self.status = format!(
                        "Done — sync anchor={anchor:.1} samples, {} frames, tiles: {}/{}",
                        self.frames_total, self.tiles_ok, self.tiles_total
                    );
                }
                DecodeMsg::Error(e) => {
                    self.decoding = false;
                    self.status = format!("Error: {e}");
                }
            }
        }
    }
}

/// Common decode runner: called by file mode, live mode and the real-time WAV playback simulation,
/// from within a thread already running in the background. Finds sync, walks frame by
/// frame, and reports each frame to the UI thread via `tx`.
///
/// `skip_frames`: SILENTLY skips the first `skip_frames` of the walk (already applied to the UI in a
/// previous pass), reporting only NEW frames. For the real-time WAV playback
/// simulation — since the buffer grows on every pass it still has to walk from the start (there is no
/// stateful API), but showing this to the UI as "from the very start" each time (resetting and
/// refilling the image/log) looks to the user as if every pass "goes back to the start and rescans".
/// In file mode `skip_frames=0`.
///
/// `known_anchor`: if given, the sync SEARCH is skipped entirely and this anchor is used directly.
/// CRITICAL for callers that repeatedly decode a growing buffer (live microphone/real-time WAV
/// playback) — re-searching on every pass could find an anchor a few samples different from pass to
/// pass, since the search range changes with the buffer, which broke the "previously decoded" frame
/// assumption matched with `skip_frames` and made the whole walk stop
/// early (see the `kgstv_modem::decode_wav_sequential_cb_from_anchor`
/// documentation — this was a bug found via user feedback).
fn run_decode(
    ctx: &egui::Context,
    tx: &Sender<DecodeMsg>,
    label: String,
    sample_rate: f64,
    samples: Arc<Vec<f64>>,
    skip_frames: usize,
    known_anchor: Option<f64>,
) {
    let _ = tx.send(DecodeMsg::Started { label, sample_rate, n_samples: samples.len() });
    ctx.request_repaint();

    let tx2 = tx.clone();
    let ctx2 = ctx.clone();
    // `Arc<AtomicUsize>` (NOT a simple `let mut seen`): since the closure is MOVED into
    // `decode_wav_sequential_cb*`, we need to be able to read the final count after the call — a shared counter is required.
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_cb = seen.clone();
    let frame_cb = move |fr| {
        let n = seen_cb.fetch_add(1, Ordering::Relaxed);
        if n >= skip_frames {
            let _ = tx2.send(DecodeMsg::Frame(fr));
            ctx2.request_repaint();
        }
    };

    let anchor = if let Some(a) = known_anchor {
        kgstv_modem::decode_wav_sequential_cb_from_anchor(&samples, sample_rate, a, 5000, frame_cb);
        a
    } else {
        // Scan the first 15 seconds, bearing in mind that the sync may not be in the first seconds
        // due to the recorder's start-up delay/silence.
        let coarse_search_len = ((15.0 * sample_rate) as i64).min(samples.len() as i64);
        kgstv_modem::decode_wav_sequential_cb(&samples, sample_rate, coarse_search_len, 5000, frame_cb)
    };
    let frames_this_pass = seen.load(Ordering::Relaxed);
    let _ = tx.send(DecodeMsg::Done { anchor, frames_this_pass });
    ctx.request_repaint();
}

/// Dropdown text for the selected audio device: `None` = system default; an id that is no longer
/// in the list (device unplugged) falls back to showing the raw id.
fn device_label(devices: &[kgstv_modem::AudioDeviceInfo], selected: Option<&str>) -> String {
    match selected {
        None => "(system default)".to_string(),
        Some(id) => devices.iter().find(|d| d.id == id).map(|d| d.label.clone()).unwrap_or_else(|| id.to_string()),
    }
}

/// Converts ASCII text into an MSB-first bit sequence (for callsign/text frames).
fn ascii_to_bits(s: &str) -> Vec<u8> {
    let mut bits = Vec::with_capacity(s.len() * 8);
    for &b in s.as_bytes() {
        for i in (0..8).rev() {
            bits.push((b >> i) & 1);
        }
    }
    bits
}

/// Converts an MSB-first bit sequence into ASCII text (inverse of `ascii_to_bits`) — to make the
/// raw data of callsign/text frames readable. Non-printable bytes are shown as `.`
/// (multi-byte encodings such as Shift-JIS are possible, but plain ASCII is assumed for now).
fn bits_to_ascii(bits: &[u8]) -> String {
    bits.chunks(8)
        .filter(|c| c.len() == 8)
        .map(|c| {
            let byte = c.iter().fold(0u8, |acc, &b| (acc << 1) | (b & 1));
            if byte.is_ascii_graphic() || byte == b' ' {
                byte as char
            } else {
                '.'
            }
        })
        .collect()
}

/// Extracts the 16×16 RGB block at tile index `(tx,ty)` from `rgba` (320×240 RGBA8, row-major)
/// (TX side — to be fed to `kgstv_core::rgb_to_tile`).
fn extract_tile_rgb(rgba: &[u8], tx: usize, ty: usize) -> [[[u8; 3]; TILE]; TILE] {
    let mut out = [[[0u8; 3]; TILE]; TILE];
    for dy in 0..TILE {
        for dx in 0..TILE {
            let px = tx * TILE + dx;
            let py = ty * TILE + dy;
            let idx = (py * IMG_W + px) * 4;
            out[dy][dx] = [rgba[idx], rgba[idx + 1], rgba[idx + 2]];
        }
    }
    out
}

/// Computes the scale that fits the 320×240 KG-STV image into the available panel area (`available`)
/// — a fixed 2.0 scale cropped the image at the bottom at the default window size and forced the user
/// to enlarge the window by hand (user feedback: "adjust the frame around the picture to the
/// screen height, I have to enlarge the window to see it fully"). Chooses the largest scale that
/// fits both height and width while preserving the aspect ratio; it does not go below 1.0 so it
/// stays legible in very small panels, and does not exceed 2.5 so it does not look needlessly huge
/// in very large windows.
fn image_fit_scale(available: egui::Vec2) -> f32 {
    let by_h = available.y / IMG_H as f32;
    let by_w = available.x / IMG_W as f32;
    by_h.min(by_w).clamp(1.0, 2.5)
}

/// Simple "LED"-style VU meter: a horizontal bar made of segments that light up from green to red
/// according to `level` (roughly 0.0-1.2, peak amplitude) — the classic audio-equipment level meter
/// look. Updated by periodically reading `LiveCapture::level()` (the UI is already redrawn
/// regularly during live capture).
fn paint_vu_meter(ui: &mut egui::Ui, level: f32) {
    const N_SEGMENTS: usize = 20;
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width().min(280.0), 18.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(15));

    let lit = ((level.clamp(0.0, 1.2) / 1.2) * N_SEGMENTS as f32).round() as usize;
    let gap = 2.0;
    let seg_w = (rect.width() - gap * (N_SEGMENTS as f32 - 1.0)) / N_SEGMENTS as f32;
    for i in 0..N_SEGMENTS {
        let x0 = rect.left() + i as f32 * (seg_w + gap);
        let seg_rect =
            egui::Rect::from_min_size(egui::pos2(x0, rect.top() + 2.0), egui::vec2(seg_w, rect.height() - 4.0));
        let on = i < lit;
        // Bottom ~65% green, next ~20% yellow, top ~15% red — classic VU meter thresholds.
        let color = if i >= N_SEGMENTS - 3 {
            if on { egui::Color32::from_rgb(220, 40, 40) } else { egui::Color32::from_rgb(60, 20, 20) }
        } else if i >= N_SEGMENTS - 7 {
            if on { egui::Color32::from_rgb(230, 200, 30) } else { egui::Color32::from_rgb(60, 50, 15) }
        } else if on {
            egui::Color32::from_rgb(60, 220, 90)
        } else {
            egui::Color32::from_rgb(20, 50, 25)
        };
        painter.rect_filled(seg_rect, 1.0, color);
    }
}

/// Draws the eye diagram (`points`: (phase∈[0,1], soft metric∈[-1,1])) inside the given rectangle.
/// Consecutive points are joined within the same "trace"; when the phase wraps
/// (goes back from 1.0 to 0.0) a new trace is started — for the classic overlapping lines
/// look.
fn paint_eye_diagram(ui: &mut egui::Ui, rect: egui::Rect, points: &[(f32, f32)]) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(18));

    let to_screen = |phase: f32, m: f32| -> egui::Pos2 {
        let x = rect.left() + phase * rect.width();
        // m: -1..1 -> y: bottom..top (m=+1 at the top)
        let y = rect.center().y - (m * 0.45) * rect.height();
        egui::pos2(x, y)
    };

    // Grid: -1, 0, +1 levels.
    for &m in &[-1.0f32, 0.0, 1.0] {
        let y = to_screen(0.0, m).y;
        painter.hline(rect.x_range(), y, egui::Stroke::new(1.0, egui::Color32::from_gray(50)));
    }
    painter.text(
        rect.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        "bit=1",
        egui::FontId::monospace(10.0),
        egui::Color32::from_gray(120),
    );
    painter.text(
        rect.left_bottom() + egui::vec2(4.0, -12.0),
        egui::Align2::LEFT_TOP,
        "bit=0",
        egui::FontId::monospace(10.0),
        egui::Color32::from_gray(120),
    );

    if points.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "(no audio)",
            egui::FontId::proportional(14.0),
            egui::Color32::from_gray(90),
        );
        return;
    }

    // LOW alpha is critical: the oscilloscope "phosphor persistence" look relies on HUNDREDS of
    // overlapping traces — each trace must be relatively TRANSPARENT so the density builds up gradually
    // into a clear gradient (frequently overlapping flat plateaus bright/opaque, sparse transition/crossing
    // regions faint). A high alpha (e.g. 43%) saturated everything to opaque green after a few traces,
    // destroying ALL the density information (and hence the crossing points) — this was the real
    // problem found via user feedback.
    let stroke = egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(80, 220, 120, 20));
    let mut trace: Vec<egui::Pos2> = Vec::new();
    let mut last_phase = -1.0f32;
    for &(phase, m) in points {
        if phase < last_phase - 0.5 {
            if trace.len() > 1 {
                painter.add(egui::Shape::line(trace.clone(), stroke));
            }
            trace.clear();
        }
        trace.push(to_screen(phase, m));
        last_phase = phase;
    }
    if trace.len() > 1 {
        painter.add(egui::Shape::line(trace, stroke));
    }
}

impl KgstvApp {
    fn persisted_settings(&self) -> PersistedSettings {
        PersistedSettings {
            tx_callsign: self.tx_callsign.clone(),
            tx_message: self.tx_message.clone(),
            tx_quality: self.tx_quality,
            tx_conv: self.tx_conv,
            tx_fsk4: self.tx_fsk4,
            tci_host: self.tci_host.clone(),
            tci_port: self.tci_port,
            tci_rx_audio: self.tci_rx_audio,
            tci_tx_audio: self.tci_tx_audio,
            tci_freq_khz: self.tci_freq_khz.clone(),
            tci_mode: self.tci_mode.clone(),
            input_device: self.selected_input_device.clone(),
            output_device: self.selected_output_device.clone(),
            input_gain: self.input_gain,
            output_gain: self.output_gain,
        }
    }

    fn current_tx_params(&self) -> TxParams {
        TxParams {
            callsign: self.tx_callsign.clone(),
            message: self.tx_message.clone(),
            quality: self.tx_quality,
            conv: self.tx_conv,
            fsk4: self.tx_fsk4,
        }
    }

    /// Discards the encoded TX audio if the callsign, message, Scale, CONV or 4L-FSK setting has
    /// changed since it was encoded (changing the image already discards it in `load_tx_image`),
    /// so "📤 Send Image" / "💾 Save WAV" never use an outdated encoding — Send Image then
    /// re-encodes automatically. Not done during playback: the progress display reads `tx_audio`.
    fn discard_stale_tx_audio(&mut self) {
        if self.tx_audio.is_none() || self.tx_playing {
            return;
        }
        if self.tx_audio_params.as_ref() != Some(&self.current_tx_params()) {
            self.tx_audio = None;
            self.tx_audio_includes_image = false;
            self.tx_audio_params = None;
            self.tx_status = "Settings changed — the image will be re-encoded when sent.".to_string();
        }
    }

    fn apply_persisted_settings(&mut self, s: PersistedSettings) {
        self.tx_callsign = s.tx_callsign;
        self.tx_message = s.tx_message;
        self.tx_quality = s.tx_quality.min(15);
        self.tx_conv = s.tx_conv;
        self.tx_fsk4 = s.tx_fsk4;
        self.tci_host = s.tci_host;
        self.tci_port = if s.tci_port == 0 { kgstv_modem::TCI_DEFAULT_PORT } else { s.tci_port };
        self.tci_rx_audio = s.tci_rx_audio;
        self.tci_tx_audio = s.tci_tx_audio;
        self.tci_freq_khz = s.tci_freq_khz;
        self.tci_mode = s.tci_mode;
        // A saved device that is not present now (unplugged, or its ID changed) falls back to the
        // system default — otherwise live decode / transmit would fail with "device not found".
        self.selected_input_device = s.input_device.filter(|id| self.input_devices.iter().any(|d| &d.id == id));
        self.selected_output_device = s.output_device.filter(|id| self.output_devices.iter().any(|d| &d.id == id));
        let gain = |g: f32| if g.is_finite() { g.clamp(0.0, 4.0) } else { 1.0 };
        self.input_gain = gain(s.input_gain);
        self.output_gain = gain(s.output_gain);
    }
}

impl eframe::App for KgstvApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.persisted_settings());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_messages();
        self.drain_tx_messages(&ctx);
        self.discard_stale_tx_audio();

        self.maybe_kick_live_decode(&ctx);
        self.maybe_kick_wav_sim_decode(&ctx);
        self.recompute_eye_diagram();
        if self.live.is_some() || self.wav_sim.is_some() || self.tx_encoding || self.tx_playing {
            // Trigger regular redraws during live/real-time playback/encoding/playing.
            ctx.request_repaint_after(Duration::from_millis(150));
        } else if self.tci.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        if self.texture_dirty {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([IMG_W, IMG_H], &self.image_rgba);
            match &mut self.texture {
                Some(tex) => tex.set(color_image, egui::TextureOptions::NEAREST),
                None => {
                    self.texture = Some(ctx.load_texture(
                        "decoded-image",
                        color_image,
                        egui::TextureOptions::NEAREST,
                    ));
                }
            }
            self.texture_dirty = false;
        }
        if self.tx_texture_dirty {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([IMG_W, IMG_H], &self.tx_image_rgba);
            match &mut self.tx_texture {
                Some(tex) => tex.set(color_image, egui::TextureOptions::NEAREST),
                None => {
                    self.tx_texture = Some(ctx.load_texture(
                        "tx-image",
                        color_image,
                        egui::TextureOptions::NEAREST,
                    ));
                }
            }
            self.tx_texture_dirty = false;
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(self.mode == Mode::Rx, "📡 RX (Receive)").clicked() {
                    self.mode = Mode::Rx;
                }
                if ui.selectable_label(self.mode == Mode::Tx, "📤 TX (Send)").clicked() {
                    self.mode = Mode::Tx;
                }
                ui.separator();

                match self.mode {
                    Mode::Rx => {
                        let busy = !self.decoding && self.live.is_none() && self.wav_sim.is_none();
                        if ui.add_enabled(busy, egui::Button::new("📂 Open WAV...")).clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                                self.open_file(&ctx, path);
                            }
                        }
                        if self.wav_sim.is_some() {
                            if ui.button("⏹ Stop Real-Time").clicked() {
                                self.wav_sim = None;
                                self.status = "Real-time playback stopped.".to_string();
                            }
                        } else if ui
                            .add_enabled(!self.decoding && self.live.is_none(), egui::Button::new("🐢 Play in Real Time"))
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                                self.start_wav_sim(path);
                            }
                        }
                        let live_label =
                            if self.live.is_some() { "⏹ Stop Live Decode" } else { "🎙 Start Live Decode" };
                        if ui.add_enabled(self.wav_sim.is_none(), egui::Button::new(live_label)).clicked() {
                            self.toggle_live(&ctx);
                        }
                        if self.decoding {
                            ui.spinner();
                        }
                        ui.separator();
                        ui.label(&self.status);
                    }
                    Mode::Tx => {
                        if ui.add_enabled(!self.tx_encoding, egui::Button::new("🖼 Choose Image...")).clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Image", &["png", "jpg", "jpeg", "gif", "bmp"])
                                .pick_file()
                            {
                                self.load_tx_image(path);
                            }
                        }
                        if ui
                            .add_enabled(!self.tx_encoding, egui::Button::new("⚙ Encode"))
                            .clicked()
                        {
                            self.start_tx_encode(&ctx, true);
                        }
                        if ui
                            .add_enabled(self.tx_audio.is_some() && !self.tx_encoding, egui::Button::new("💾 Save WAV"))
                            .clicked()
                        {
                            self.save_tx_wav();
                        }
                        // Sends the full transmission: callsign + message (if filled in) + the 300 image
                        // tiles. Plays the encoded audio if it already includes the image; otherwise
                        // (nothing encoded yet, or only text from "📨 Send Text") it encodes with the
                        // image first and then plays automatically.
                        let image_audio_ready = self.tx_audio.is_some() && self.tx_audio_includes_image;
                        if ui
                            .add_enabled(
                                !self.tx_encoding
                                    && !self.tx_playing
                                    && (image_audio_ready || self.tx_source_name.is_some()),
                                egui::Button::new("📤 Send Image"),
                            )
                            .on_hover_text("Transmits the image and the text (callsign and message).")
                            .on_disabled_hover_text("Choose an image first.")
                            .clicked()
                        {
                            if image_audio_ready {
                                self.play_tx(&ctx);
                            } else {
                                self.tx_auto_play_after_encode = true;
                                self.start_tx_encode(&ctx, true);
                            }
                        }
                        // A transmission (especially a full image) can take minutes — user
                        // request: a button that can cut playback off early.
                        if ui.add_enabled(self.tx_playing, egui::Button::new("⏹ Stop")).clicked() {
                            self.stop_tx_playback();
                        }
                        if self.tx_encoding || self.tx_playing {
                            ui.spinner();
                        }
                        ui.separator();
                        ui.label(&self.tx_status);
                    }
                }
            });
        });

        match self.mode {
            Mode::Rx => {
                egui::Panel::left("log").resizable(true).default_size(420.0).show(ui, |ui| {
                    ui.heading("Frame Log");
                    ui.label(format!("Source: {}", self.current_file.as_deref().unwrap_or("(none)")));
                    ui.label(format!(
                        "Frame: {}   Tile: {}/{}",
                        self.frames_total, self.tiles_ok, self.tiles_total
                    ));
                    ui.label(
                        egui::RichText::new(format!(
                            "📻 Callsign: {}",
                            self.last_callsign.as_deref().unwrap_or("(none yet)")
                        ))
                        .strong(),
                    );
                    if let Some(text) = &self.last_text {
                        ui.label(egui::RichText::new(format!("💬 Text: {text}")).strong());
                    }
                    ui.separator();
                    self.tci_panel(ui);
                    ui.collapsing("🎚 Audio Input", |ui| {
                        ui.horizontal(|ui| {
                            let devices = self.input_devices.clone();
                            egui::ComboBox::from_label("Device")
                                .selected_text(device_label(&devices, self.selected_input_device.as_deref()))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.selected_input_device, None, "(system default)");
                                    for d in &devices {
                                        ui.selectable_value(&mut self.selected_input_device, Some(d.id.clone()), &d.label);
                                    }
                                });
                            // Rescan the device list — so a USB sound card plugged in/removed later
                            // shows up without restarting the program (user request). If the selected
                            // device is no longer in the list (it may have been removed), fall back to
                            // the system default.
                            if ui.button("🔄").on_hover_text("Rescan devices").clicked() {
                                self.input_devices = kgstv_modem::list_input_devices();
                                if let Some(sel) = &self.selected_input_device {
                                    if !self.input_devices.iter().any(|d| &d.id == sel) {
                                        self.selected_input_device = None;
                                    }
                                }
                            }
                        });
                        ui.add(egui::Slider::new(&mut self.input_gain, 0.0..=4.0).text("Input level"));
                        if let Some(live) = &self.live {
                            live.set_gain(self.input_gain);
                        }
                        let level = self.live.as_ref().map(|l| l.level()).unwrap_or(0.0);
                        paint_vu_meter(ui, level);
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(
                        ui,
                        |ui| {
                            for line in &self.log_lines {
                                ui.monospace(line);
                            }
                        },
                    );
                });
            }
            Mode::Tx => {
                egui::Panel::left("txpanel").resizable(true).default_size(420.0).show(ui, |ui| {
                    ui.heading("Transmit Settings");
                    ui.label(format!("Source: {}", self.tx_source_name.as_deref().unwrap_or("(none selected)")));
                    ui.separator();
                    ui.label("Callsign (optional):");
                    ui.text_edit_singleline(&mut self.tx_callsign);
                    ui.label("Message (optional):");
                    ui.text_edit_multiline(&mut self.tx_message);
                    // User request: "add a send button next to the text box..
                    // when I press the button it should both encode and play" — a shortcut that reduces the
                    // "⚙ Encode" + "📤 Send Image" pair above to a SINGLE step. Later correction:
                    // "the send next to the text box should send only the text" — this button
                    // does NOT INCLUDE the 300-tile image, it only encodes the callsign (if any) + text
                    // (`include_image=false`) — a quick text-only send shortcut.
                    if ui
                        .add_enabled(!self.tx_encoding && !self.tx_playing, egui::Button::new("📨 Send Text"))
                        .on_hover_text("Sends only the callsign and message (not the image). Use 📤 Send Image to send the image as well.")
                        .clicked()
                    {
                        self.tx_auto_play_after_encode = true;
                        self.start_tx_encode(&ctx, false);
                    }
                    ui.add_space(8.0);
                    // EXACTLY the same as the "Scale" drop-down list in the original Windows sender
                    // (see the `TX_SCALE_LABELS` documentation) — corrected via user feedback:
                    // there used to be a raw 1-15 slider here, and for a user who knows the real program
                    // the "scale" values (0.07…2.0) were meaningless.
                    ui.horizontal(|ui| {
                        ui.label("Scale:");
                        egui::ComboBox::from_id_salt("tx_scale")
                            .selected_text(TX_SCALE_LABELS[self.tx_quality as usize])
                            .show_ui(ui, |ui| {
                                for (i, label) in TX_SCALE_LABELS.iter().enumerate() {
                                    ui.selectable_value(&mut self.tx_quality, i as u8, *label);
                                }
                            });
                    });
                    ui.checkbox(&mut self.tx_conv, "Convolutional coding (CONV)");
                    ui.checkbox(&mut self.tx_fsk4, "4L-FSK modulation (for text+image — callsign is always MSK)");
                    ui.add_space(8.0);
                    let (done, total) = self.tx_progress;
                    if total > 0 {
                        ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(format!("{done}/{total}")));
                    }
                    ui.separator();
                    ui.label("300 tiles (20×15), 1200 baud MSK — same framing as real KG-STV.");
                    ui.separator();
                    self.tci_panel(ui);
                    ui.collapsing("🎚 Audio Output", |ui| {
                        ui.horizontal(|ui| {
                            let devices = self.output_devices.clone();
                            egui::ComboBox::from_label("Device")
                                .selected_text(device_label(&devices, self.selected_output_device.as_deref()))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.selected_output_device, None, "(system default)");
                                    for d in &devices {
                                        ui.selectable_value(&mut self.selected_output_device, Some(d.id.clone()), &d.label);
                                    }
                                });
                            // See the comment on the same button on the input side.
                            if ui.button("🔄").on_hover_text("Rescan devices").clicked() {
                                self.output_devices = kgstv_modem::list_output_devices();
                                if let Some(sel) = &self.selected_output_device {
                                    if !self.output_devices.iter().any(|d| &d.id == sel) {
                                        self.selected_output_device = None;
                                    }
                                }
                            }
                        });
                        ui.add(egui::Slider::new(&mut self.output_gain, 0.0..=4.0).text("Output level"));
                    });
                });
            }
        }

        egui::Panel::bottom("eye").resizable(true).default_size(340.0).min_size(160.0).show(ui, |ui| {
            ui.heading("Eye Diagram — instantaneous frequency discriminator");
            let (rect, _resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ui.available_height()),
                egui::Sense::hover(),
            );
            paint_eye_diagram(ui, rect, &self.eye_points);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.mode {
            Mode::Rx => {
                ui.heading("Image (320×240)");
                if let Some(tex) = &self.texture {
                    // An adaptive scale based on the available panel area instead of a fixed 2.0 — at the
                    // default window size the image was cropped and the user had to enlarge the window
                    // by hand (user feedback: "adjust the frame around the picture to the screen
                    // height, I have to enlarge the window to see it
                    // fully").
                    let scale = image_fit_scale(ui.available_size());
                    ui.add(
                        egui::Image::new(tex)
                            .fit_to_exact_size(egui::vec2(IMG_W as f32 * scale, IMG_H as f32 * scale)),
                    );
                }
            }
            Mode::Tx => {
                ui.heading("Image to Send (320×240)");
                // Progress bar while playing — user request: "shall we add a progress bar
                // showing percentage complete". Time-based (instead of sample-precise tracking — `play_blocking`
                // may resample when the device requires it, but the duration does not change).
                if self.tx_playing {
                    if let Some((sr, samples)) = &self.tx_audio {
                        let elapsed = self.tx_play_started.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
                        let total = (samples.len() as f64 / sr).max(1e-6);
                        let frac = (elapsed / total).clamp(0.0, 1.0);
                        ui.add(egui::ProgressBar::new(frac as f32).text(format!("sending — {:.0}%", frac * 100.0)));
                    }
                }
                if let Some(tex) = &self.tx_texture {
                    let scale = image_fit_scale(ui.available_size());
                    let resp = ui.add(
                        egui::Image::new(tex)
                            .fit_to_exact_size(egui::vec2(IMG_W as f32 * scale, IMG_H as f32 * scale)),
                    );
                    // Mark the tiles sent so far on the image — user
                    // request: "let's mark the sent part on the picture for clarity". Tiles NOT YET
                    // SENT are darkened, so the "revealed" part shows what has been sent
                    // (a scan/reveal effect).
                    if self.tx_playing {
                        let sent = self.tx_tiles_sent_so_far();
                        let painter = ui.painter_at(resp.rect);
                        let tile_px = TILE as f32 * scale;
                        for ty in 0..TILES_Y {
                            for tx_ in 0..TILES_X {
                                if sent.contains(&(tx_ as u8, ty as u8)) {
                                    continue;
                                }
                                let x0 = resp.rect.left() + tx_ as f32 * tile_px;
                                let y0 = resp.rect.top() + ty as f32 * tile_px;
                                let r = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(tile_px, tile_px));
                                painter.rect_filled(r, 0.0, egui::Color32::from_black_alpha(165));
                            }
                        }
                    }
                }
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // A height of 800 cropped the image with the default panel sizes (top toolbar + 2×-scaled
            // 480px image + eye-diagram panel) (see the
            // `image_fit_scale` documentation) — increased to 960 so the image fits fully in the
            // default window too (the adaptive scale already guarantees it is never cropped in smaller
            // windows; this just aims for a better default).
            .with_inner_size([1150.0, 960.0])
            .with_title("Atölye m-KGSTV (with mods by G0ORX)"),
        ..Default::default()
    };
    // Optional: if invoked as `kgstv-gui <recording.wav>`, automatically loads that file at
    // startup (handy for quick testing/scripting without dealing with the native file picker).
    let cli_wav_path = std::env::args().nth(1).map(PathBuf::from);
    eframe::run_native(
        // App ID: also names the folder eframe saves the persisted settings in — keep it unchanged
        // so existing saved settings are still found (the window title is set separately above).
        "Atölye m-KGSTV",
        options,
        Box::new(move |cc| {
            let mut app = KgstvApp::default();
            if let Some(settings) =
                cc.storage.and_then(|st| eframe::get_value::<PersistedSettings>(st, SETTINGS_KEY))
            {
                app.apply_persisted_settings(settings);
            }
            if let Some(path) = cli_wav_path {
                app.open_file(&cc.egui_ctx, path);
            }
            Ok(Box::new(app))
        }),
    )
}
