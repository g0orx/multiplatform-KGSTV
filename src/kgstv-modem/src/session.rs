//! Üst seviye oturum (session) API'si: bir ses kaydını KG-STV frame'lerine decode etmek,
//! ya da bir dizi frame'i (çağrı işareti + görüntü tile'ları) ses kaydına encode etmek.
//! Bit-seviyesi protokol mantığı tamamen `kgstv_core::frame`'e devredilir; bu modül sadece
//! ses <-> bit dönüşümünü (MSK demod/mod + senkron arama) protokolle birleştirir.

use crate::filter::bandpass_filtfilt;
use crate::fsk4::{self, Fsk4Detector};
use crate::msk::{modulate_msk, ToneClock};
use crate::sync::{find_sync, SYNC_LEN};
use kgstv_core::constants::PREAMBLE_LEN;
use kgstv_core::header::InfoChunk;
use kgstv_core::{frame, Command};

/// 4L-FSK veri chunk'ı çözümünde denenecek global faz ofseti (radyan) sayısı — bilinmeyen
/// mutlak taşıyıcı fazını telafi etmek için [0, 60°) aralığını tarar (bkz. `fsk4.rs`
/// modül dokümantasyonu, "6 durumlu CPM trellis"). CRC geçen ilk ofsette erken çıkılır.
const FSK4_PHASE_SEARCH_STEPS: usize = 16;

/// 4L-FSK için ince zamanlama kayması taraması (örnek cinsinden, `anchor`'a eklenir).
/// Gerçek kayıtlarda, MSK'nin (büyüklük-bazlı, faz-uyumsuz) demodülasyonuna tamamen görünmez
/// kalan birkaç örneklik bir sabit kayma bulundu (bkz. docs/protocol_spec.md, "4L-FSK ince
/// zamanlama kayması") — faz-uyumlu (coherent) Viterbi korelasyonu buna çok duyarlı. Bu kayma
/// muhtemelen tüm dosya için SABİT (kayıt zincirinin grup gecikmesi); bu yüzden
/// `decode_wav_sequential_cb` ilk başarılı 4L-FSK chunk'ından sonra bulduğu kaymayı bir
/// "ipucu" olarak sonraki chunk'lara taşır (önce sadece ipucunu dener, hızlı yol).
const FSK4_SHIFT_SEARCH: [f64; 33] = [
    -8.0, -7.5, -7.0, -6.5, -6.0, -5.5, -5.0, -4.5, -4.0, -3.5, -3.0, -2.5, -2.0, -1.5, -1.0, -0.5, 0.0, 0.5, 1.0,
    1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0, 7.5, 8.0,
];

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

/// MSK (2-seviyeli) veri chunk'ı çözer. `n`: kodlanmış+whitened bit sayısı.
fn decode_data_msk(clk: &ToneClock, anchor: f64, pos: i64, n: i64, size_bits: usize, conv: bool) -> (Vec<u8>, bool) {
    let rx: Vec<u8> = (0..n).map(|i| clk.msk_bit(anchor, pos + i)).collect();
    if conv {
        frame::decode_data_conv(&rx, size_bits)
    } else {
        frame::decode_data_norm(&rx, size_bits)
    }
}

/// 4L-FSK veri chunk'ı çözer: bilinmeyen mutlak taşıyıcı fazını [0,60°) aralığında, ve bilinmeyen
/// ince zamanlama kaymasını (bkz. `FSK4_SHIFT_SEARCH`) tarayıp, her adayı gerçek CRC ile
/// doğrulayarak en iyisini seçer (bkz. `fsk4.rs`). `n`: kodlanmış+whitened bit sayısı —
/// sembol sayısı `ceil(n/2)`'dir (her sembol 2 bit taşır). `shift_hint`: önceki chunk'ta
/// bulunan kayma (önce bu denenir — genelde tüm dosya için sabit olduğundan hızlı yoldur).
/// Dönüş: (veri, crc_ok, kullanılan kayma).
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
            continue; // zaten denendi
        }
        if let Some((data, ok)) = try_shift(shift) {
            return (data, ok, shift);
        }
    }
    (Vec::new(), false, shift_hint)
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
    // Bir 4L-FSK chunk'ında bulunan ince zamanlama kayması, sonraki 4L-FSK chunk'ları için
    // ipucu olarak taşınır (bkz. `FSK4_SHIFT_SEARCH` dokümantasyonu — genelde tüm dosya için
    // sabit bir kayma, dolayısıyla ilk bulunuşundan sonra taramayı büyük ölçüde hızlandırır).
    let mut fsk4_shift_hint = 0.0f64;
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
        // Zaman ekseninde kullanılan "slot" sayısı: MSK'de her kodlanmış BİT bir slot (1200
        // baud); 4L-FSK'de her SEMBOL bir slot ve her sembol 2 bit taşır — bu yüzden slot
        // sayısı `enc_len` değil `ceil(enc_len/2)`'dir. Bunu karıştırmak sonraki frame'in
        // senkron kelimesini yanlış konumda aramaya (ve tüm oturumun kaymasına) yol açar.
        let slots_used = if fsk4_mode { (enc_len + 1) / 2 } else { enc_len };
        on_frame(FrameResult { header: hdr, header_metric: metric, data_bits: Some(data_bits), data_crc_ok });
        pos = data_start + slots_used + SYNC_LEN as i64;
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
    // Preamble/senkron/header HER ZAMAN MSK (bkz. protocol_spec.md: "Bilgi chunk'ı, veri
    // chunk'ının kendi modülasyon/kodlama ayarından bağımsız olarak her zaman MSK modülasyonu
    // + konvolüsyonel kodlama ile gönderilir") — sadece veri chunk'ı `hdr.m` alanına göre MSK
    // ya da 4L-FSK ile modüle edilir. Bu yüzden tüm oturumu tek bir homojen bit dizisi olarak
    // biriktirip bir kerede modüle EDEMEYİZ (MSK ve 4L-FSK'nin sembol/örnek eşlemesi farklı) —
    // her segment kendi modülatörüyle sese çevrilip arka arkaya eklenir. Segment sınırlarında
    // faz sıfırlanır; hem `msk::ToneClock` (büyüklük-bazlı) hem `fsk4::Fsk4Detector`
    // (faz-ofseti taramalı) alıcıları mutlak faza bağlı olmadığından bu decode doğruluğunu
    // ETKİLEMEZ.
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
        on_progress(i + 1, total);
    }
    // Normal tamamlanma: bitiş sinyali 3 kez (bkz. protocol_spec.md, çerçeveleme kuralları).
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

    /// TX -> RX uçtan uca, 4L-FSK veri chunk'ı: preamble/senkron/header her zaman MSK (bkz.
    /// `encode_session_audio_cb` dokümantasyonu), ama METİN veri chunk'ı `m=1` ile 4L-FSK
    /// olarak modüle edilip Viterbi/MLSD demodülatörüyle geri çözülüyor. Hem NORM hem CONV
    /// kodlama kombinasyonlarını dener.
    #[test]
    fn full_session_roundtrip_fsk4_text_norm_and_conv() {
        let sr = 44100.0;
        for &conv in &[0u8, 1u8] {
            let text = b"MERHABA 4L-FSK DUNYASI".to_vec();
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

            assert!(!frames.is_empty(), "conv={conv}: en az bir frame bulunmalı");
            let text_frame = &frames[0];
            assert_eq!(text_frame.header.com, Command::Text as u8);
            assert_eq!(text_frame.header.m, 1);
            assert!(text_frame.data_crc_ok, "conv={conv}: 4L-FSK veri chunk'ı CRC hatalı");
            assert_eq!(text_frame.data_bits.as_ref().unwrap(), &text_bits, "conv={conv}");

            // Bir sonraki frame'in (END) doğru konumda bulunması, 4L-FSK slot/pos
            // muhasebesinin (sembol sayısı = ceil(bit/2)) doğru olduğunu kanıtlıyor —
            // yanlış olsaydı senkron kelimesi yanlış yerde aranır, oturum burada dururdu.
            assert!(frames.len() >= 2, "conv={conv}: END frame'i bulunamadı — pos muhasebesi kaymış olabilir");
            assert_eq!(frames[1].header.com, Command::End as u8);
        }
    }

    /// TX -> RX uçtan uca, 4L-FSK ile GÖRÜNTÜ tile'ları — `kgstv-gui`'nin TX panelindeki
    /// "4L-FSK modülasyonu" seçeneğinin kullandığı yolun tam bir aynası
    /// (`full_session_image_roundtrip_solid_tiles`'ın 4L-FSK karşılığı).
    #[test]
    fn full_session_image_roundtrip_fsk4() {
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
            assert!(fr.data_crc_ok, "4L-FSK tile {i} CRC hatalı");
            let bits = fr.data_bits.as_ref().unwrap();
            let tile = kgstv_core::decode_tile(bits, fr.header.sc).expect("tile decode başarısız");
            let rgb = kgstv_core::tile_to_rgb(&tile);
            let [r, g, b] = rgb[8][8];
            let [er, eg, eb] = colors[i];
            assert!((r as i32 - er as i32).abs() < 40, "r={r} beklenen={er}");
            assert!((g as i32 - eg as i32).abs() < 40, "g={g} beklenen={eg}");
            assert!((b as i32 - eb as i32).abs() < 40, "b={b} beklenen={eb}");
        }
    }

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
