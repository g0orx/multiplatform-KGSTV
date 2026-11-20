//! End-to-end TCI test against an in-process mock TCI server that behaves like piHPSDR:
//! sends its initial state + `ready;`, streams RX audio (float32 stereo, `length` = total values)
//! after `audio_start:0;`, and while keyed with `trx:0,true,tci;` sends header-only TX_CHRONO
//! blocks (length 2048, 2 channels) at real-time pace, collecting the left channel of each
//! TX_AUDIO_STREAM reply. Both directions must carry a real KG-STV session that still decodes.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use kgstv_core::{Command, InfoChunk};
use kgstv_modem::{decode_wav_sequential, encode_session_audio, TciClient, TCI_AUDIO_SAMPLE_RATE};
use tungstenite::Message;

fn text_session(text: &[u8], sample_rate: f64) -> Vec<f64> {
    let bits: Vec<u8> = text.iter().flat_map(|&b| (0..8).rev().map(move |i| (b >> i) & 1)).collect();
    let hdr = InfoChunk {
        sys: 0,
        com: Command::Text as u8,
        c: 1,
        m: 0,
        x: 0,
        y: 0,
        sc: 10,
        size: bits.len() as u16,
        crc: 0,
    };
    encode_session_audio(&[(hdr, bits)], sample_rate)
}

fn decoded_text(samples: &[f64], sample_rate: f64) -> Option<String> {
    let (_, frames) = decode_wav_sequential(samples, sample_rate, (sample_rate * 2.0) as i64, 10);
    let fr = frames.iter().find(|f| f.header.com == Command::Text as u8 && f.data_crc_ok)?;
    let bytes: Vec<u8> =
        fr.data_bits.as_ref()?.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b)).collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn header(length: u32, kind: u32) -> Vec<u8> {
    let mut h = Vec::new();
    for f in [0u32, 48000, 3, 0, 0, length, kind, 2] {
        h.extend_from_slice(&f.to_le_bytes());
    }
    h.resize(64, 0);
    h
}

/// Mock server; returns the left channel of all TX audio received.
fn run_mock_server(listener: TcpListener, rx_audio: Vec<f64>) -> Vec<f32> {
    let (stream, _) = listener.accept().unwrap();
    let mut ws = tungstenite::accept(stream).unwrap();
    ws.send(Message::text("protocol:ExpertSDR3,2.0;device:MockSDR;vfo:0,0,14233000;modulation:0,usb;ready;start;"))
        .unwrap();
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(2))).unwrap();
    let mut tx_audio = Vec::new();
    let mut keyed = false;
    let mut was_keyed = false;
    let mut next_chrono = Instant::now();
    let mut rx_pos: Option<usize> = None;
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                for cmd in t.as_str().split(';').map(str::trim).filter(|c| !c.is_empty()) {
                    match cmd {
                        "audio_start:0" => rx_pos = Some(0),
                        "trx:0,true,tci" => {
                            keyed = true;
                            was_keyed = true;
                            ws.send(Message::text("trx:0,true;")).unwrap();
                        }
                        "trx:0,false" => {
                            keyed = false;
                            ws.send(Message::text("trx:0,false;")).unwrap();
                        }
                        c if c.starts_with("vfo:0,0,") => ws.send(Message::text(format!("{c};"))).unwrap(),
                        _ => {}
                    }
                }
            }
            Ok(Message::Binary(b)) => {
                let length = u32::from_le_bytes(b[20..24].try_into().unwrap()) as usize;
                let kind = u32::from_le_bytes(b[24..28].try_into().unwrap());
                assert_eq!(kind, 2, "client may only send TX audio blocks");
                assert_eq!(b.len(), 64 + length * 4, "TX block must hold exactly `length` floats");
                // Same as piHPSDR: `length` total values, take left, skip right.
                for pair in b[64..].chunks_exact(8) {
                    tx_audio.push(f32::from_le_bytes(pair[0..4].try_into().unwrap()));
                }
            }
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        // Stream RX audio in 1024-frame stereo blocks (as fast as possible — the client buffers).
        if let Some(pos) = rx_pos.as_mut() {
            if *pos < rx_audio.len() {
                let end = (*pos + 1024).min(rx_audio.len());
                let mut blk = header(((end - *pos) * 2) as u32, 1);
                for &v in &rx_audio[*pos..end] {
                    blk.extend_from_slice(&(v as f32).to_le_bytes());
                    blk.extend_from_slice(&(v as f32).to_le_bytes());
                }
                ws.send(Message::binary(blk)).unwrap();
                *pos = end;
            }
        }
        // While keyed, request 1024 stereo frames every 1024/48000 s, like piHPSDR.
        if keyed && Instant::now() >= next_chrono {
            ws.send(Message::binary(header(2048, 3))).unwrap();
            next_chrono += Duration::from_secs_f64(1024.0 / 48000.0);
        } else if !keyed {
            next_chrono = Instant::now();
        }
        if was_keyed && !keyed {
            break;
        }
    }
    tx_audio
}

#[test]
fn tci_rx_and_tx_audio_carry_a_decodable_kgstv_session() {
    let msg = b"TCI LOOPBACK DE G0ORX";
    let rx_signal = text_session(msg, TCI_AUDIO_SAMPLE_RATE);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || run_mock_server(listener, rx_signal));

    let client = TciClient::connect("127.0.0.1", port);
    let t0 = Instant::now();
    while !(client.is_connected() && client.status().ready) {
        assert!(t0.elapsed() < Duration::from_secs(5), "no connection: {:?}", client.status().state);
        std::thread::sleep(Duration::from_millis(10));
    }
    let st = client.status();
    assert_eq!(st.device.as_deref(), Some("MockSDR"));
    assert_eq!(st.frequency_hz, Some(14_233_000));
    assert_eq!(st.modulation.as_deref(), Some("usb"));

    // Control: tuning is echoed back by the server and reflected in the status.
    client.set_frequency(7_173_000);
    let t0 = Instant::now();
    while client.status().frequency_hz != Some(7_173_000) {
        assert!(t0.elapsed() < Duration::from_secs(2), "frequency change not reflected");
        std::thread::sleep(Duration::from_millis(10));
    }

    // RX: the radio's audio stream must decode to the transmitted text.
    let capture = client.start_rx_audio(60.0, 1.0).unwrap();
    let expected_len = text_session(msg, TCI_AUDIO_SAMPLE_RATE).len();
    let t0 = Instant::now();
    while capture.len() < expected_len {
        assert!(t0.elapsed() < Duration::from_secs(10), "RX audio stalled at {}", capture.len());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(capture.sample_rate, TCI_AUDIO_SAMPLE_RATE);
    assert_eq!(decoded_text(&capture.snapshot(), capture.sample_rate).as_deref(), Some("TCI LOOPBACK DE G0ORX"));
    drop(capture);

    // TX: generated at 44.1 kHz (as the GUI does) — must be resampled, keyed, paced and unkeyed.
    let tx_signal = text_session(msg, 44100.0);
    client.transmit_blocking(&tx_signal, 44100.0, 1.0, None).unwrap();
    let tx_audio: Vec<f64> = server.join().unwrap().into_iter().map(|v| v as f64).collect();
    assert!(tx_audio.len() as f64 >= tx_signal.len() as f64 * 48000.0 / 44100.0, "TX audio truncated");
    assert_eq!(decoded_text(&tx_audio, 48000.0).as_deref(), Some("TCI LOOPBACK DE G0ORX"));
    client.disconnect();
}
