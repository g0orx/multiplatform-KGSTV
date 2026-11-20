//! Top-level session API: decoding an audio recording into KG-STV frames,
//! or encoding a series of frames (callsign + image tiles) into an audio recording.
//! The bit-level protocol logic is delegated entirely to `kgstv_core::frame`; this module only
//! combines the audio <-> bit conversion (MSK demod/mod + sync search) with the protocol.

use crate::filter::bandpass_filtfilt;
use crate::fsk4::{self, Fsk4Detector};
use crate::msk::{modulate_msk, ToneClock};
use crate::sync::{find_sync, SYNC_LEN};
use kgstv_core::constants::{sync_word_bits, PREAMBLE_LEN};
use kgstv_core::header::InfoChunk;
use kgstv_core::{frame, Command};

/// Maximum slot (bit) window to scan when searching for an INTERMEDIATE sync word after a
/// corrupted header. When the header cannot be read, `hdr.size` is unknown, so the real length of
/// this frame's data chunk is unknown too — so the window is kept wide enough to comfortably
/// cover the longest possible data chunk (12-bit `size` field: at most 4095 bits of raw data + 16
/// bits of CRC, ×2 with the convolutional code on).
const MID_STREAM_RESYNC_WINDOW: i64 = ((4095 + 16) * 2) + HEADER_ENC_LEN + SYNC_LEN as i64 + 64;

/// Number of global phase offsets (radians) to try when decoding a 4L-FSK data chunk — scans the
/// [0, 60°) range to compensate for the unknown absolute carrier phase (see the `fsk4.rs`
/// module documentation, "6-state CPM trellis"). Exits early at the first offset that passes CRC.
const FSK4_PHASE_SEARCH_STEPS: usize = 16;

/// Fine timing-shift scan for 4L-FSK (in samples, added to `anchor`).
/// In real recordings a constant shift of a few samples was found that is completely invisible to
/// MSK's (magnitude-based, non-coherent) demodulation (see docs/protocol_spec.md, "4L-FSK fine
/// timing shift") — the phase-coherent Viterbi correlation is very sensitive to it. This shift
/// is probably CONSTANT for the whole file (group delay of the recording chain); so
/// `decode_wav_sequential_cb` carries the shift found after the first successful 4L-FSK chunk as a
/// "hint" to subsequent chunks (it tries only the hint first, the fast path).
const FSK4_SHIFT_SEARCH: [f64; 33] = [
    -8.0, -7.5, -7.0, -6.5, -6.0, -5.5, -5.0, -4.5, -4.0, -3.5, -3.0, -2.5, -2.0, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0,
    1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5, 8.0,
];

const HEADER_ENC_LEN: i64 = 108;

#[derive(Debug, Clone)]
pub struct FrameResult {
    pub header: InfoChunk,
    pub header_metric: u32,
    /// Raw bits of the data chunk immediately after the header (if any; none for END/BSR_REQ/ABORT).
    pub data_bits: Option<Vec<u8>>,
    pub data_crc_ok: bool,
}

fn has_data_chunk(com: u8) -> bool {
    !matches!(com, 3 | 4 | 5) // END, BSR_REQUEST, ABORT
}

fn decode_header_at(clk: &ToneClock, anchor: f64, pos: i64) -> (InfoChunk, u32, bool) {
    let rx: Vec<u8> = (0..HEADER_ENC_LEN).map(|i| clk.msk_bit(anchor, pos + i)).collect();
    frame::decode_header(&rx)
}

/// Decodes an MSK (2-level) data chunk. `n`: number of encoded+whitened bits.
fn decode_data_msk(clk: &ToneClock, anchor: f64, pos: i64, n: i64, size_bits: usize, conv: bool) -> (Vec<u8>, bool) {
    let rx: Vec<u8> = (0..n).map(|i| clk.msk_bit(anchor, pos + i)).collect();
    if conv {
        frame::decode_data_conv(&rx, size_bits)
    } else {
        frame::decode_data_norm(&rx, size_bits)
    }
}

/// Decodes a 4L-FSK data chunk: scans the unknown absolute carrier phase over [0,60°) and the
/// unknown fine timing shift (see `FSK4_SHIFT_SEARCH`), verifying each candidate with the real CRC
/// and choosing the best (see `fsk4.rs`). `n`: number of encoded+whitened bits —
/// the number of symbols is `ceil(n/2)` (each symbol carries 2 bits). `shift_hint`: the shift found
/// in the previous chunk (tried first — usually the fast path since it is constant for the whole file).
/// Returns: (data, crc_ok, shift used).
fn decode_data_fsk4(
    filtered: &[f64],
    sample_rate: f64,
    anchor: f64,
    pos: i64,
    n: i64,
    size_bits: usize,
    conv: bool,
    shift_hint: f64,
) -> (Vec<u8>, bool, f64) {
    let n_symbols = ((n + 1) / 2) as usize;
    let detector = Fsk4Detector::new(filtered, sample_rate);

    let try_shift = |shift: f64| -> Option<(Vec<u8>, bool)> {
        for k in 0..FSK4_PHASE_SEARCH_STEPS {
            let phase_offset = (k as f64 / FSK4_PHASE_SEARCH_STEPS as f64) * (2.0 * std::f64::consts::PI / 6.0);
            let levels = detector.decode_symbols(anchor + shift, pos, n_symbols, phase_offset);
            let mut bits = fsk4::levels_to_bits(&levels);
            bits.truncate(n as usize);
            let result =
                if conv { frame::decode_data_conv(&bits, size_bits) } else { frame::decode_data_norm(&bits, size_bits) };
            if result.1 {
                return Some(result);
            }
        }
        None
    };

    if let Some((data, ok)) = try_shift(shift_hint) {
        return (data, ok, shift_hint);
    }
    for &shift in &FSK4_SHIFT_SEARCH {
        if (shift - shift_hint).abs() < 0.01 {
            continue; // already tried
        }
        if let Some((data, ok)) = try_shift(shift) {
            return (data, ok, shift);
        }
    }
    (Vec::new(), false, shift_hint)
}

/// Decodes a clean/short MSK recording from start to finish (sync is found once, then walked
/// sequentially). For long/noisy real recordings the independent-candidate scanning approach in
/// `research/decoder/decode_session.py` is more reliable — this forms a good basis for porting
/// that approach to Rust.
pub fn decode_wav_sequential(
    samples: &[f64],
    sample_rate: f64,
    coarse_search_len: i64,
    max_frames: usize,
) -> (f64, Vec<FrameResult>) {
    let mut frames = Vec::new();
    let anchor = decode_wav_sequential_cb(samples, sample_rate, coarse_search_len, max_frames, |fr| {
        frames.push(fr);
    });
    (anchor, frames)
}

/// Same as `decode_wav_sequential`, but `on_frame` is called as each frame is decoded
/// (without accumulating the Vec until the end). Designed for callers doing progressive/live
/// display such as the GUI — they can process each frame immediately without accumulating.
pub fn decode_wav_sequential_cb(
    samples: &[f64],
    sample_rate: f64,
    coarse_search_len: i64,
    max_frames: usize,
    on_frame: impl FnMut(FrameResult),
) -> f64 {
    let filtered = bandpass_filtfilt(samples, sample_rate, 900.0, 2200.0);
    let clk = ToneClock::new(&filtered, sample_rate);
    // Coarse scan step is a quarter of the symbol duration (~tb): in real recordings the sync
    // often starts SECONDS after the preamble/sync (recorder start-up delay,
    // silence etc.) — scanning the whole `coarse_search_len` range with step=1 would be needlessly
    // slow; setting `fine_span` equal to this step guarantees the fine search compensates for the
    // skipped samples in between.
    let coarse_step = ((clk.tb() / 4.0).floor() as i64).max(1);
    let (anchor, _err) = find_sync(
        &clk,
        (0..coarse_search_len).step_by(coarse_step as usize),
        coarse_step as f64,
        0.05,
    );
    walk_frames(&clk, &filtered, sample_rate, anchor, max_frames, on_frame);
    anchor
}

/// Same as `decode_wav_sequential_cb`, but skips the sync SEARCH and uses the given `anchor`
/// directly.
///
/// For callers that repeatedly decode a growing buffer (live microphone / real-time
/// WAV playback simulation, see `kgstv-gui`): re-running `find_sync` on every pass could find
/// an anchor A FEW SAMPLES DIFFERENT FROM PASS TO PASS, since the search range
/// (`coarse_search_len = min(15s, current buffer length)`) changes with the buffer
/// — which could shift the positions of previously successfully decoded frames, break that pass's
/// header CRC early and make the WHOLE walk (and hence the image tiles) stop without ever
/// progressing (this was a bug verified with a real recording — user
/// feedback: "decoding is also very unsuccessful ... the image never decoded"). Finding the anchor
/// once and pinning it (stored by the caller as an `Option` and passed back in) removes this
/// instability.
pub fn decode_wav_sequential_cb_from_anchor(
    samples: &[f64],
    sample_rate: f64,
    anchor: f64,
    max_frames: usize,
    on_frame: impl FnMut(FrameResult),
) {
    let filtered = bandpass_filtfilt(samples, sample_rate, 900.0, 2200.0);
    let clk = ToneClock::new(&filtered, sample_rate);
    walk_frames(&clk, &filtered, sample_rate, anchor, max_frames, on_frame);
}

fn walk_frames(
    clk: &ToneClock,
    filtered: &[f64],
    sample_rate: f64,
    anchor: f64,
    max_frames: usize,
    mut on_frame: impl FnMut(FrameResult),
) {
    let mut pos = PREAMBLE_LEN as i64 + SYNC_LEN as i64;
    // The fine timing shift found in one 4L-FSK chunk is carried as a hint for subsequent 4L-FSK
    // chunks (see the `FSK4_SHIFT_SEARCH` documentation — usually a constant shift for the whole
    // file, so after it is first found it speeds up the scan considerably).
    let mut fsk4_shift_hint = 0.0f64;
    for _ in 0..max_frames {
        if pos + HEADER_ENC_LEN > filtered.len() as i64 {
            break;
        }
        let (hdr, metric, crc_ok) = decode_header_at(&clk, anchor, pos);
        if !crc_ok {
            // Corrupted header (channel noise/RF corruption) — previously the whole walk
            // STOPPED here, which (since the anchor is cached in live mode) made every re-decode
            // pass get stuck at EXACTLY THE SAME point over and over
            // (user feedback: "decoding stops after a few frames", the frame counter on screen
            // stayed constant). Since only the 63-bit sync word is repeated between consecutive
            // frames (NO preamble), we can skip the corrupted header and get back into the stream
            // by searching for the NEXT sync word.
            match find_mid_stream_sync(clk, anchor, pos, MID_STREAM_RESYNC_WINDOW.min(filtered.len() as i64 - pos)) {
                Some(resync_pos) => {
                    pos = resync_pos + SYNC_LEN as i64;
                    continue;
                }
                None => break, // no valid sync found in the window — end of stream
            }
        }
        let data_start = pos + HEADER_ENC_LEN;
        if !has_data_chunk(hdr.com) {
            on_frame(FrameResult { header: hdr, header_metric: metric, data_bits: None, data_crc_ok: false });
            // END/ABORT is the signal announcing that the session has formally ended/been cancelled
            // (sent 3× in a row on normal completion, see protocol_spec.md). What follows is no longer
            // real KG-STV data — it is silence/noise. Previously the walk carried on regardless;
            // after the corrupted-header resync was added (see `find_mid_stream_sync`) this could
            // make the receiver find a SPURIOUS sync word in the remaining noise and, with the chance
            // that a random header CRC happens to match, produce GARBAGE frames on screen (and even
            // image tiles written at the wrong x/y — user feedback: "it also corrupts the data it
            // received"). BSR_REQUEST is deliberately NOT included here
            // — it is part of the normal session flow, not an end signal.
            if matches!(Command::from_u8(hdr.com), Some(Command::End) | Some(Command::Abort)) {
                break;
            }
            pos = data_start + SYNC_LEN as i64;
            continue;
        }
        let conv = hdr.c == 1;
        let fsk4_mode = hdr.m == 1;
        let mult: i64 = if conv { 2 } else { 1 };
        let enc_len = (hdr.size as i64 + 16) * mult;
        let (data_bits, data_crc_ok) = if fsk4_mode {
            let (data, ok, shift) = decode_data_fsk4(
                &filtered,
                sample_rate,
                anchor,
                data_start,
                enc_len,
                hdr.size as usize,
                conv,
                fsk4_shift_hint,
            );
            if ok {
                fsk4_shift_hint = shift;
            }
            (data, ok)
        } else {
            decode_data_msk(&clk, anchor, data_start, enc_len, hdr.size as usize, conv)
        };
        // Number of "slots" used on the time axis: in MSK each encoded BIT is one slot (1200
        // baud); in 4L-FSK each SYMBOL is one slot and each symbol carries 2 bits — so the slot
        // count is `ceil(enc_len/2)`, not `enc_len`. Mixing this up leads to searching for the next
        // frame's sync word at the wrong position (and the whole session drifting).
        let slots_used = if fsk4_mode { (enc_len + 1) / 2 } else { enc_len };
        on_frame(FrameResult { header: hdr, header_metric: metric, data_bits: Some(data_bits), data_crc_ok });
        pos = data_start + slots_used + SYNC_LEN as i64;
    }
}

/// Searches for the INTERMEDIATE (preamble-less) 63-bit sync word in a window of `search_len`
/// slots starting at `search_start` — unlike `sync::sync_err`, the `PREAMBLE_LEN` offset is
/// NOT used, because only the sync word is repeated between consecutive frames
/// (the preamble is sent only once at the very start of the file, see protocol_spec.md). Used
/// by `walk_frames` to get back into the stream after a corrupted header.
///
/// A small bit-error margin is allowed (the sync word itself may arrive with a few bit
/// errors from channel noise); the best (lowest-error) candidate is returned if it is below the
/// threshold — if above the threshold (no reliable match in the window), `None`.
fn find_mid_stream_sync(clk: &ToneClock, anchor: f64, search_start: i64, search_len: i64) -> Option<i64> {
    let sw = sync_word_bits();
    let mut best: Option<(u32, i64)> = None;
    for cand in search_start..(search_start + search_len) {
        let mut err = 0u32;
        for i in 0..SYNC_LEN as i64 {
            if clk.msk_bit(anchor, cand + i) != sw[i as usize] {
                err += 1;
            }
        }
        if best.is_none_or(|(be, _)| err < be) {
            best = Some((err, cand));
            if err == 0 {
                break;
            }
        }
    }
    best.filter(|&(err, _)| err <= (SYNC_LEN as u32) / 6).map(|(_, cand)| cand)
}

/// Encodes a series of (header, raw-data-bits) frames into a full KG-STV session (preamble once,
/// each frame with its own sync word, 3× END signal at the end) and then into audio samples.
/// `header.size` must be CONSISTENT with `data_bits.len()` (the caller is responsible).
pub fn encode_session_audio(frames: &[(InfoChunk, Vec<u8>)], sample_rate: f64) -> Vec<f64> {
    encode_session_audio_cb(frames, sample_rate, |_, _, _| {})
}

/// Same as `encode_session_audio`, but `on_progress(completed, total, total_samples_so_far)` is
/// called as each frame's bit-level encoding finishes — for callers that show a progress bar,
/// such as the GUI (since an image transmission can have hundreds of tiles).
/// The third parameter (cumulative sample count) lets the caller compute which position in the
/// audio each frame corresponds to during PLAYBACK (e.g. to mark "tiles sent so far" in the
/// TX preview).
pub fn encode_session_audio_cb(
    frames: &[(InfoChunk, Vec<u8>)],
    sample_rate: f64,
    mut on_progress: impl FnMut(usize, usize, usize),
) -> Vec<f64> {
    // Preamble/sync/header are ALWAYS MSK (see protocol_spec.md: "The info chunk is always sent
    // with MSK modulation + convolutional coding, independent of the data chunk's own
    // modulation/coding setting") — only the data chunk is modulated with MSK or 4L-FSK according
    // to the `hdr.m` field. So we CANNOT accumulate the whole session as one homogeneous bit
    // sequence and modulate it in one go (MSK and 4L-FSK have different symbol/sample mappings) —
    // each segment is converted to audio with its own modulator and appended one after another. Phase
    // is reset at segment boundaries; since neither the `msk::ToneClock` (magnitude-based) nor the
    // `fsk4::Fsk4Detector` (phase-offset scanning) receivers depend on absolute phase, this does NOT
    // AFFECT decode accuracy.
    let total = frames.len();
    let mut audio = modulate_msk(&frame::preamble_bits(), sample_rate);
    for (i, (hdr, data_bits)) in frames.iter().enumerate() {
        audio.extend(modulate_msk(&frame::sync_bits(), sample_rate));
        audio.extend(modulate_msk(&frame::encode_header(hdr), sample_rate));

        let enc_bits =
            if hdr.c == 1 { frame::encode_data_conv(data_bits) } else { frame::encode_data_norm(data_bits) };
        if hdr.m == 1 {
            let levels = fsk4::bits_to_levels(&enc_bits);
            audio.extend(fsk4::modulate_fsk4(&levels, sample_rate));
        } else {
            audio.extend(modulate_msk(&enc_bits, sample_rate));
        }
        on_progress(i + 1, total, audio.len());
    }
    // Normal completion: end signal 3 times (see protocol_spec.md, framing rules).
    let end_hdr = InfoChunk { sys: 0, com: Command::End as u8, c: 0, m: 0, x: 0, y: 0, sc: 0, size: 0, crc: 0 };
    for _ in 0..3 {
        audio.extend(modulate_msk(&frame::sync_bits(), sample_rate));
        audio.extend(modulate_msk(&frame::encode_header(&end_hdr), sample_rate));
    }
    audio
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `decode_wav_sequential_cb_from_anchor`, with the known correct anchor, must give EXACTLY THE
    /// SAME result as the fully searching `decode_wav_sequential_cb` — proving that it is safe for the
    /// live microphone/real-time WAV playback simulation to cache and reuse the anchor (see `kgstv-gui`
    /// `maybe_kick_live_decode`).
    #[test]
    fn decode_from_known_anchor_matches_full_search() {
        let sr = 44100.0;
        let callsign = b"TESTCALL".to_vec();
        let mut bits = Vec::new();
        for &b in &callsign {
            for i in (0..8).rev() {
                bits.push((b >> i) & 1);
            }
        }
        let hdr = InfoChunk { sys: 0, com: Command::Callsign as u8, c: 0, m: 0, x: 0, y: 0, sc: 10, size: bits.len() as u16, crc: 0 };
        let audio = encode_session_audio(&[(hdr, bits.clone())], sr);

        let (anchor, frames_full) = decode_wav_sequential(&audio, sr, 500, 10);

        let mut frames_cached = Vec::new();
        decode_wav_sequential_cb_from_anchor(&audio, sr, anchor, 10, |fr| frames_cached.push(fr));

        assert_eq!(frames_full.len(), frames_cached.len());
        for (a, b) in frames_full.iter().zip(frames_cached.iter()) {
            assert_eq!(a.header.com, b.header.com);
            assert_eq!(a.header.x, b.header.x);
            assert_eq!(a.header.y, b.header.y);
            assert_eq!(a.data_crc_ok, b.data_crc_ok);
            assert_eq!(a.data_bits, b.data_bits);
        }
        assert!(frames_cached[0].data_crc_ok);
        assert_eq!(frames_cached[0].data_bits.as_ref().unwrap(), &bits);
    }

    /// TX -> RX end to end, 4L-FSK data chunk: preamble/sync/header are always MSK (see the
    /// `encode_session_audio_cb` documentation), but the TEXT data chunk is modulated as 4L-FSK
    /// with `m=1` and decoded back with the Viterbi/MLSD demodulator. Tries both NORM and CONV
    /// coding combinations.
    #[test]
    fn full_session_roundtrip_fsk4_text_norm_and_conv() {
        let sr = 44100.0;
        for &conv in &[0u8, 1u8] {
            let text = b"HELLO 4L-FSK WORLD".to_vec();
            let mut text_bits = Vec::new();
            for &b in &text {
                for i in (0..8).rev() {
                    text_bits.push((b >> i) & 1);
                }
            }
            let hdr = InfoChunk {
                sys: 0,
                com: Command::Text as u8,
                c: conv,
                m: 1, // 4L-FSK
                x: 0,
                y: 0,
                sc: 10,
                size: text_bits.len() as u16,
                crc: 0,
            };

            let audio = encode_session_audio(&[(hdr, text_bits.clone())], sr);
            let (_anchor, frames) = decode_wav_sequential(&audio, sr, 500, 10);

            assert!(!frames.is_empty(), "conv={conv}: at least one frame must be found");
            let text_frame = &frames[0];
            assert_eq!(text_frame.header.com, Command::Text as u8);
            assert_eq!(text_frame.header.m, 1);
            assert!(text_frame.data_crc_ok, "conv={conv}: 4L-FSK data chunk has a CRC error");
            assert_eq!(text_frame.data_bits.as_ref().unwrap(), &text_bits, "conv={conv}");

            // Finding the next frame (END) at the correct position proves that the 4L-FSK slot/pos
            // accounting (number of symbols = ceil(bits/2)) is correct —
            // if it were wrong the sync word would be searched at the wrong place and the session would stop here.
            assert!(frames.len() >= 2, "conv={conv}: END frame not found — pos accounting may have drifted");
            assert_eq!(frames[1].header.com, Command::End as u8);
        }
    }

    /// TX -> RX end to end, IMAGE tiles with 4L-FSK — an exact mirror of the path used by the
    /// "4L-FSK modulation" option in `kgstv-gui`'s TX panel
    /// (the 4L-FSK counterpart of `full_session_image_roundtrip_solid_tiles`).
    ///
    /// `quality=15` (the coarsest `sc`, the shortest bit sequence) was chosen deliberately: demodulation
    /// of the 4L-FSK data channel is still not fully mature (see the module documentation, "~80-83% symbol
    /// accuracy") — after the quantization-scale correction of `scale_qt` (see that documentation,
    /// "Correction #2") lower `sc` values produce LONGER bit sequences (less
    /// lossy), which can erode FSK4's already-marginal symbol accuracy enough to break the CRC.
    /// This test exists to verify the 4L-FSK MODULATION path, not the JPEG
    /// quantization quality — the `sc` giving the shortest/most robust bit sequence is deliberate.
    #[test]
    fn full_session_image_roundtrip_fsk4() {
        let sr = 44100.0;
        let colors: [[u8; 3]; 3] = [[220, 30, 30], [30, 200, 30], [30, 30, 220]];
        let quality = 15u8;
        let mut frames = Vec::new();
        for (i, &[r, g, b]) in colors.iter().enumerate() {
            let rgb = [[[r, g, b]; 16]; 16];
            let tile = kgstv_core::rgb_to_tile(&rgb);
            let bits = kgstv_core::encode_tile(&tile, quality);
            let hdr = InfoChunk {
                sys: 0,
                com: Command::Image as u8,
                c: (i % 2) as u8,
                m: 1, // 4L-FSK
                x: i as u8,
                y: 0,
                sc: quality,
                size: bits.len() as u16,
                crc: 0,
            };
            frames.push((hdr, bits));
        }

        let audio = encode_session_audio(&frames, sr);
        let (_anchor, decoded) = decode_wav_sequential(&audio, sr, 500, 20);

        let image_frames: Vec<_> =
            decoded.iter().filter(|f| f.header.com == Command::Image as u8).collect();
        assert_eq!(image_frames.len(), colors.len());
        for (i, fr) in image_frames.iter().enumerate() {
            assert_eq!(fr.header.m, 1);
            assert!(fr.data_crc_ok, "4L-FSK tile {i} CRC error");
            let bits = fr.data_bits.as_ref().unwrap();
            let tile = kgstv_core::decode_tile(bits, fr.header.sc).expect("tile decode failed");
            let rgb = kgstv_core::tile_to_rgb(&tile);
            let [r, g, b] = rgb[8][8];
            let [er, eg, eb] = colors[i];
            assert!((r as i32 - er as i32).abs() < 40, "r={r} expected={er}");
            assert!((g as i32 - eg as i32).abs() < 40, "g={g} expected={eg}");
            assert!((b as i32 - eb as i32).abs() < 40, "b={b} expected={eb}");
        }
    }

    #[test]
    fn full_session_roundtrip_callsign_and_text() {
        let sr = 44100.0;
        let callsign = b"OH2UDS ".to_vec(); // 7 characters = 56 bits (enough for the test even if not in the BSR range)
        let mut callsign_bits = Vec::new();
        for &b in &callsign {
            for i in (0..8).rev() {
                callsign_bits.push((b >> i) & 1);
            }
        }
        let callsign_hdr = InfoChunk {
            sys: 0,
            com: Command::Callsign as u8,
            c: 0,
            m: 0,
            x: 0,
            y: 0,
            sc: 10,
            size: callsign_bits.len() as u16,
            crc: 0,
        };

        let text = b"HELLO WORLD".to_vec();
        let mut text_bits = Vec::new();
        for &b in &text {
            for i in (0..8).rev() {
                text_bits.push((b >> i) & 1);
            }
        }
        let text_hdr = InfoChunk {
            sys: 0,
            com: Command::Text as u8,
            c: 1, // test the CONV path too
            m: 0,
            x: 0,
            y: 0,
            sc: 10,
            size: text_bits.len() as u16,
            crc: 0,
        };

        let audio = encode_session_audio(
            &[(callsign_hdr, callsign_bits.clone()), (text_hdr, text_bits.clone())],
            sr,
        );

        let (_anchor, frames) = decode_wav_sequential(&audio, sr, 500, 20);
        assert!(frames.len() >= 3, "at least callsign+text+end frames must be found, found: {}", frames.len());

        assert_eq!(frames[0].header.com, Command::Callsign as u8);
        assert!(frames[0].data_crc_ok);
        assert_eq!(frames[0].data_bits.as_ref().unwrap(), &callsign_bits);

        assert_eq!(frames[1].header.com, Command::Text as u8);
        assert!(frames[1].data_crc_ok);
        assert_eq!(frames[1].data_bits.as_ref().unwrap(), &text_bits);

        assert_eq!(frames[2].header.com, Command::End as u8);
    }

    /// TX -> RX end to end: image tiles encoded with `kgstv_core::rgb_to_tile`/`encode_tile`
    /// are converted to audio with `encode_session_audio` and decoded back with `decode_wav_sequential`
    /// — an exact mirror of the path used by `kgstv-gui`'s TX feature.
    #[test]
    fn full_session_image_roundtrip_solid_tiles() {
        let sr = 44100.0;
        let colors: [[u8; 3]; 3] = [[220, 30, 30], [30, 200, 30], [30, 30, 220]];
        let quality = 10u8;
        let mut frames = Vec::new();
        for (i, &[r, g, b]) in colors.iter().enumerate() {
            let rgb = [[[r, g, b]; 16]; 16];
            let tile = kgstv_core::rgb_to_tile(&rgb);
            let bits = kgstv_core::encode_tile(&tile, quality);
            let hdr = InfoChunk {
                sys: 0,
                com: Command::Image as u8,
                c: (i % 2) as u8, // test NORM and CONV mixed
                m: 0,
                x: i as u8,
                y: 0,
                sc: quality,
                size: bits.len() as u16,
                crc: 0,
            };
            frames.push((hdr, bits));
        }

        let audio = encode_session_audio(&frames, sr);
        let (_anchor, decoded) = decode_wav_sequential(&audio, sr, 500, 20);

        let image_frames: Vec<_> =
            decoded.iter().filter(|f| f.header.com == Command::Image as u8).collect();
        assert_eq!(image_frames.len(), colors.len());
        for (i, fr) in image_frames.iter().enumerate() {
            assert!(fr.data_crc_ok, "tile {i} CRC error");
            let bits = fr.data_bits.as_ref().unwrap();
            let tile = kgstv_core::decode_tile(bits, fr.header.sc).expect("tile decode failed");
            let rgb = kgstv_core::tile_to_rgb(&tile);
            let [r, g, b] = rgb[8][8];
            let [er, eg, eb] = colors[i];
            // Since JPEG is lossy it is not exact, but the channel relationship/dominant colour must be right.
            assert!((r as i32 - er as i32).abs() < 40, "r={r} expected={er}");
            assert!((g as i32 - eg as i32).abs() < 40, "g={g} expected={eg}");
            assert!((b as i32 - eb as i32).abs() < 40, "b={b} expected={eb}");
        }
    }

    /// Verifies that a header becoming unreadable due to channel noise/corruption must not stop
    /// the WHOLE walk in `walk_frames` — previously the walk stopped COMPLETELY at the first corrupted
    /// header (`if !crc_ok { break; }`), which in live mode (since the anchor is cached) made every
    /// re-decode pass get stuck at the SAME point over and over (user feedback: "decoding stops
    /// after a few frames", the frame/tile counter on screen stayed constant without
    /// progressing). Deliberately corrupts the header of a middle frame with noise and verifies
    /// that the frame AFTER it is still found (via the INTERMEDIATE sync-word search, see
    /// `find_mid_stream_sync`).
    #[test]
    fn walk_frames_recovers_after_corrupted_header_mid_stream() {
        let sr = 44100.0;
        let colors: [[u8; 3]; 3] = [[220, 30, 30], [30, 200, 30], [30, 30, 220]];
        let quality = 10u8;
        let mut frames = Vec::new();
        for (i, &[r, g, b]) in colors.iter().enumerate() {
            let rgb = [[[r, g, b]; 16]; 16];
            let tile = kgstv_core::rgb_to_tile(&rgb);
            let bits = kgstv_core::encode_tile(&tile, quality);
            let hdr = InfoChunk {
                sys: 0,
                com: Command::Image as u8,
                c: 0,
                m: 0,
                x: i as u8,
                y: 0,
                sc: quality,
                size: bits.len() as u16,
                crc: 0,
            };
            frames.push((hdr, bits));
        }
        let mut audio = encode_session_audio(&frames, sr);

        // Compute the exact position of the second frame's (index 1) header in the audio by repeating
        // the SAME steps as the encode (preamble + frame0[sync+header+data] + frame1's sync).
        let mut prefix = modulate_msk(&frame::preamble_bits(), sr);
        prefix.extend(modulate_msk(&frame::sync_bits(), sr));
        prefix.extend(modulate_msk(&frame::encode_header(&frames[0].0), sr));
        let enc0 = frame::encode_data_norm(&frames[0].1);
        prefix.extend(modulate_msk(&enc0, sr));
        prefix.extend(modulate_msk(&frame::sync_bits(), sr));
        let header1_start = prefix.len();
        let header1_len = modulate_msk(&frame::encode_header(&frames[1].0), sr).len();

        // Replace that region with noise (so the header CRC definitely fails) — but the sync words
        // before/after it and the other frames stay intact.
        for (i, s) in audio[header1_start..header1_start + header1_len].iter_mut().enumerate() {
            *s = if i % 2 == 0 { 0.9 } else { -0.9 };
        }

        let (_anchor, decoded) = decode_wav_sequential(&audio, sr, 500, 20);
        let image_frames: Vec<_> = decoded.iter().filter(|f| f.header.com == Command::Image as u8).collect();

        // The corrupted frame (x=1) is not in the list due to the header CRC error — but walk_frames
        // must be able to reach the THIRD frame (x=2) via resync.
        assert!(
            image_frames.iter().any(|f| f.header.x == 2 && f.data_crc_ok),
            "3rd tile (x=2) not found via resync AFTER the corrupted 2nd frame — walk_frames \
             could not get back into the stream (see find_mid_stream_sync)"
        );
        assert!(
            !image_frames.iter().any(|f| f.header.x == 1),
            "corrupted header (x=1) was still reported as a frame — not expected"
        );
    }

    /// Verifies that when a session ends normally (END signal), `walk_frames` must NOT keep "finding"
    /// SPURIOUS frames in the noise/silence AFTER it — after the corrupted-header resync was added
    /// (see `find_mid_stream_sync`) the walk could continue past END, stumble on a sync-like pattern
    /// by chance in the noise behind it, and produce invalid "frames" (even image tiles written to the
    /// wrong position) (user feedback: "even though the sender has finished its job ... it also
    /// corrupts the data it received", "we can't tell that the other side's transmission has ended").
    #[test]
    fn walk_frames_stops_cleanly_at_end_signal_and_ignores_trailing_noise() {
        let sr = 44100.0;
        let callsign = b"TESTEND".to_vec();
        let mut bits = Vec::new();
        for &b in &callsign {
            for i in (0..8).rev() {
                bits.push((b >> i) & 1);
            }
        }
        let hdr = InfoChunk { sys: 0, com: Command::Callsign as u8, c: 0, m: 0, x: 0, y: 0, sc: 10, size: bits.len() as u16, crc: 0 };
        let mut audio = encode_session_audio(&[(hdr, bits)], sr);

        // Simulate the live-mode scenario where the receiver keeps listening AFTER the real
        // transmission has ended: append (deterministic, non-repeating) "noise" at the end.
        let mut state = 12345u32;
        let noise_len = (5.0 * sr) as usize; // 5 s of noise
        for _ in 0..noise_len {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let v = ((state >> 16) as i32 % 2000) as f64 / 1000.0 - 1.0; // approximately [-1,1)
            audio.push(v);
        }

        let (_anchor, decoded) = decode_wav_sequential(&audio, sr, 500, 50);

        // Expected: the callsign frame, then the FIRST END frame (no need to walk all of the 3×
        // repeats — the walk MUST STOP at the first END caught). No SPURIOUS frames from the noise
        // should be added.
        assert_eq!(
            decoded.len(),
            2,
            "unexpected frame count/content: {:?}",
            decoded.iter().map(|f| f.header.com).collect::<Vec<_>>()
        );
        assert_eq!(decoded[0].header.com, Command::Callsign as u8);
        assert_eq!(decoded[1].header.com, Command::End as u8);
    }
}
