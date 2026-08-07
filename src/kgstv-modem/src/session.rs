//! Üst seviye oturum (session) API'si: bir ses kaydını KG-STV frame'lerine decode etmek,
//! ya da bir dizi frame'i (çağrı işareti + görüntü tile'ları) ses kaydına encode etmek.
//! Bit-seviyesi protokol mantığı tamamen `kgstv_core::frame`'e devredilir; bu modül sadece
//! ses <-> bit dönüşümünü (MSK demod/mod + senkron arama) protokolle birleştirir.

use crate::filter::bandpass_filtfilt;
use crate::msk::{modulate_msk, ToneClock};
use crate::sync::{find_sync, SYNC_LEN};
use kgstv_core::constants::PREAMBLE_LEN;
use kgstv_core::header::InfoChunk;
use kgstv_core::{frame, Command};

const HEADER_ENC_LEN: i64 = 108;

#[derive(Debug, Clone)]
pub struct FrameResult {
    pub header: InfoChunk,
    pub header_metric: u32,
    /// Header'ın hemen sonrasındaki veri chunk'ının ham bitleri (varsa; END/BSR_REQ/ABORT'ta yok).
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

fn decode_data_at(clk: &ToneClock, anchor: f64, pos: i64, size_bits: usize, conv: bool) -> (Vec<u8>, bool) {
    let mult: i64 = if conv { 2 } else { 1 };
    let n = (size_bits as i64 + 16) * mult;
    let rx: Vec<u8> = (0..n).map(|i| clk.msk_bit(anchor, pos + i)).collect();
    if conv {
        frame::decode_data_conv(&rx, size_bits)
    } else {
        frame::decode_data_norm(&rx, size_bits)
    }
}

/// Temiz/kısa bir MSK kaydını (senkron bir kez bulunup, ardından sıralı olarak yürünür)
/// baştan sona decode eder. Uzun/gürültülü gerçek kayıtlar için `research/decoder/
/// decode_session.py`'deki bağımsız-aday-tarama yaklaşımı daha güvenilirdir — bu, o
/// yaklaşımın Rust'a taşınması için iyi bir temel oluşturur.
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

/// `decode_wav_sequential` ile aynı, ama her frame decode edildikçe `on_frame` çağrılır
/// (Vec'i sonuna kadar biriktirmeden). GUI gibi ilerlemeli/canlı gösterim yapan
/// çağıranlar için tasarlandı — her frame'i biriktirmeden hemen işleme alabilirler.
pub fn decode_wav_sequential_cb(
    samples: &[f64],
    sample_rate: f64,
    coarse_search_len: i64,
    max_frames: usize,
    mut on_frame: impl FnMut(FrameResult),
) -> f64 {
    let filtered = bandpass_filtfilt(samples, sample_rate, 900.0, 2200.0);
    let clk = ToneClock::new(&filtered, sample_rate);
    // Kaba tarama adımı sembol süresinin (~tb) dörtte biri: gerçek kayıtlarda senkron
    // genellikle preamble/sync'ten SANİYELER sonra başlar (kayıt cihazının başlatma gecikmesi,
    // sessizlik vb.) — tüm `coarse_search_len` aralığını adım=1 ile taramak gereksiz yavaş
    // olurdu; `fine_span`'i bu adıma eşitlemek, atlanan aradaki örnekleri ince aramanın telafi
    // etmesini garanti eder.
    let coarse_step = ((clk.tb() / 4.0).floor() as i64).max(1);
    let (anchor, _err) = find_sync(
        &clk,
        (0..coarse_search_len).step_by(coarse_step as usize),
        coarse_step as f64,
        0.05,
    );

    let mut pos = PREAMBLE_LEN as i64 + SYNC_LEN as i64;
    for _ in 0..max_frames {
        if pos + HEADER_ENC_LEN > filtered.len() as i64 {
            break;
        }
        let (hdr, metric, crc_ok) = decode_header_at(&clk, anchor, pos);
        if !crc_ok {
            break;
        }
        let data_start = pos + HEADER_ENC_LEN;
        if !has_data_chunk(hdr.com) {
            on_frame(FrameResult { header: hdr, header_metric: metric, data_bits: None, data_crc_ok: false });
            pos = data_start + SYNC_LEN as i64;
            continue;
        }
        let conv = hdr.c == 1;
        let (data_bits, data_crc_ok) = decode_data_at(&clk, anchor, data_start, hdr.size as usize, conv);
        let mult: i64 = if conv { 2 } else { 1 };
        let enc_len = (hdr.size as i64 + 16) * mult;
        on_frame(FrameResult { header: hdr, header_metric: metric, data_bits: Some(data_bits), data_crc_ok });
        pos = data_start + enc_len + SYNC_LEN as i64;
    }
    anchor
}

/// Bir dizi (header, ham-veri-bitleri) frame'ini tam bir KG-STV oturumuna (preamble bir kez,
/// her frame kendi senkron kelimesiyle, sonda 3× END sinyali) ve ardından ses örneklerine
/// encode eder. `header.size`, `data_bits.len()` ile TUTARLI olmalıdır (çağıran sorumludur).
pub fn encode_session_audio(frames: &[(InfoChunk, Vec<u8>)], sample_rate: f64) -> Vec<f64> {
    encode_session_audio_cb(frames, sample_rate, |_, _| {})
}

/// `encode_session_audio` ile aynı, ama her frame'in bit-düzeyi kodlaması bitince
/// `on_progress(tamamlanan, toplam)` çağrılır — GUI gibi ilerleme çubuğu gösteren
/// çağıranlar için (görüntü gönderiminde yüzlerce tile olabileceğinden).
pub fn encode_session_audio_cb(
    frames: &[(InfoChunk, Vec<u8>)],
    sample_rate: f64,
    mut on_progress: impl FnMut(usize, usize),
) -> Vec<f64> {
    let total = frames.len();
    let mut bits = frame::preamble_bits();
    for (i, (hdr, data_bits)) in frames.iter().enumerate() {
        bits.extend(frame::sync_bits());
        bits.extend(frame::encode_header(hdr));
        if hdr.c == 1 {
            bits.extend(frame::encode_data_conv(data_bits));
        } else {
            bits.extend(frame::encode_data_norm(data_bits));
        }
        on_progress(i + 1, total);
    }
    // Normal tamamlanma: bitiş sinyali 3 kez (bkz. protocol_spec.md, çerçeveleme kuralları).
    let end_hdr = InfoChunk { sys: 0, com: Command::End as u8, c: 0, m: 0, x: 0, y: 0, sc: 0, size: 0, crc: 0 };
    for _ in 0..3 {
        bits.extend(frame::sync_bits());
        bits.extend(frame::encode_header(&end_hdr));
    }
    modulate_msk(&bits, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_session_roundtrip_callsign_and_text() {
        let sr = 44100.0;
        let callsign = b"OH2UDS ".to_vec(); // 7 karakter = 56 bit (BSR aralığında olmasa da test için yeterli)
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

        let text = b"MERHABA DUNYA".to_vec();
        let mut text_bits = Vec::new();
        for &b in &text {
            for i in (0..8).rev() {
                text_bits.push((b >> i) & 1);
            }
        }
        let text_hdr = InfoChunk {
            sys: 0,
            com: Command::Text as u8,
            c: 1, // CONV yolunu da test et
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
        assert!(frames.len() >= 3, "en az callsign+text+end frame'i bulunmalı, bulunan: {}", frames.len());

        assert_eq!(frames[0].header.com, Command::Callsign as u8);
        assert!(frames[0].data_crc_ok);
        assert_eq!(frames[0].data_bits.as_ref().unwrap(), &callsign_bits);

        assert_eq!(frames[1].header.com, Command::Text as u8);
        assert!(frames[1].data_crc_ok);
        assert_eq!(frames[1].data_bits.as_ref().unwrap(), &text_bits);

        assert_eq!(frames[2].header.com, Command::End as u8);
    }

    /// TX -> RX uçtan uca: `kgstv_core::rgb_to_tile`/`encode_tile` ile encode edilmiş görüntü
    /// tile'ları, `encode_session_audio` ile ses'e çevrilip `decode_wav_sequential` ile geri
    /// çözülüyor — `kgstv-gui`'nin TX özelliğinin kullandığı yolun tam bir aynası.
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
                c: (i % 2) as u8, // NORM ve CONV karışık test et
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
            assert!(fr.data_crc_ok, "tile {i} CRC hatalı");
            let bits = fr.data_bits.as_ref().unwrap();
            let tile = kgstv_core::decode_tile(bits, fr.header.sc).expect("tile decode başarısız");
            let rgb = kgstv_core::tile_to_rgb(&tile);
            let [r, g, b] = rgb[8][8];
            let [er, eg, eb] = colors[i];
            // JPEG kayıplı olduğu için birebir değil, ama kanal ilişkisi/dominant renk doğru olmalı.
            assert!((r as i32 - er as i32).abs() < 40, "r={r} beklenen={er}");
            assert!((g as i32 - eg as i32).abs() < 40, "g={g} beklenen={eg}");
            assert!((b as i32 - eb as i32).abs() < 40, "b={b} beklenen={eb}");
        }
    }
}
