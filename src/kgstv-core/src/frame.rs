//! Üst seviye frame encode/decode: preamble+sync (sadece transmisyon başında) ve
//! ardışık frame'ler için sync+header(+data). Gerçek off-air kayıt üzerinde frame-walking
//! ile doğrulandı (bkz. docs/protocol_spec.md, "Tam Transmisyon Yürüyüşü").

use crate::conv::{encode as conv_encode, viterbi_decode};
use crate::constants::{sync_word_bits, HEADER_ENC_BITS};
use crate::crc::crc16_aug_ccitt;
use crate::header::InfoChunk;
use crate::whiten::whiten;

/// Bir header'ı (54 ham bit) tam kodlanmış+whitened 108-bit dizisine çevirir (TX tarafı).
pub fn encode_header(hdr: &InfoChunk) -> Vec<u8> {
    let raw = hdr.to_bits();
    let encoded = conv_encode(&raw);
    whiten(&encoded)
}

/// 108-bit kodlanmış+whitened diziden header'ı çözer (RX tarafı).
/// Dönüş: (header, viterbi_metric, crc_ok).
pub fn decode_header(rx108: &[u8]) -> (InfoChunk, u32, bool) {
    assert_eq!(rx108.len(), HEADER_ENC_BITS);
    let dew = whiten(rx108); // whiten kendi tersidir
    let (raw, metric, _end_state) = viterbi_decode(&dew, 54, 0);
    let hdr = InfoChunk::parse(&raw);
    let crc_ok = hdr.crc_ok(&raw);
    (hdr, metric, crc_ok)
}

/// Ham veriyi (NORM: kodsuz) whitened data-chunk bitstream'e çevirir: data + 16-bit CRC.
pub fn encode_data_norm(data_bits: &[u8]) -> Vec<u8> {
    let crc = crc16_aug_ccitt(data_bits);
    let mut combined = data_bits.to_vec();
    combined.extend(crc_to_bits(crc));
    whiten(&combined)
}

/// NORM (kodsuz) whitened data-chunk'ı çözer. `size_bits`: beklenen ham veri uzunluğu.
/// Dönüş: (ham veri bitleri, crc_ok).
pub fn decode_data_norm(rx: &[u8], size_bits: usize) -> (Vec<u8>, bool) {
    let dew = whiten(rx);
    let data = dew[..size_bits].to_vec();
    let crc_bits = &dew[size_bits..size_bits + 16];
    let crc_val = bits_to_u16(crc_bits);
    let ok = crc16_aug_ccitt(&data) == crc_val;
    (data, ok)
}

/// Ham veriyi (CONV: K=7 konvolüsyonel kodlu) whitened data-chunk bitstream'ine çevirir:
/// data + 16-bit CRC, ardından konvolüsyonel kod + whitening (decode_data_conv'un tersi).
pub fn encode_data_conv(data_bits: &[u8]) -> Vec<u8> {
    let crc = crc16_aug_ccitt(data_bits);
    let mut combined = data_bits.to_vec();
    combined.extend(crc_to_bits(crc));
    let encoded = conv_encode(&combined);
    whiten(&encoded)
}

/// CONV (konvolüsyonel kodlu) whitened data-chunk'ı çözer.
pub fn decode_data_conv(rx: &[u8], size_bits: usize) -> (Vec<u8>, bool) {
    let dew = whiten(rx);
    let (bits, _metric, _end_state) = viterbi_decode(&dew, size_bits + 16, 0);
    let data = bits[..size_bits].to_vec();
    let crc_val = bits_to_u16(&bits[size_bits..size_bits + 16]);
    let ok = crc16_aug_ccitt(&data) == crc_val;
    (data, ok)
}

fn crc_to_bits(v: u16) -> Vec<u8> {
    (0..16).rev().map(|i| ((v >> i) & 1) as u8).collect()
}

fn bits_to_u16(bits: &[u8]) -> u16 {
    bits.iter().fold(0u16, |acc, &b| (acc << 1) | b as u16)
}

pub fn preamble_bits() -> Vec<u8> {
    (0..crate::constants::PREAMBLE_LEN as u32).map(|i| (i % 2) as u8).collect()
}

pub fn sync_bits() -> Vec<u8> {
    sync_word_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_frame_roundtrip_matches_real_capture() {
        // KG-STV_MSK_text.mp3'ten gerçek çözülen header + "TEST3>..." metni.
        let hdr = InfoChunk { sys: 0, com: 0, c: 0, m: 0, x: 0, y: 0, sc: 10, size: 480, crc: 0 };
        let enc = encode_header(&hdr);
        assert_eq!(enc.len(), 108);
        let (decoded_hdr, metric, crc_ok) = decode_header(&enc);
        assert_eq!(metric, 0);
        assert!(crc_ok);
        assert_eq!(decoded_hdr.com, 0);
        assert_eq!(decoded_hdr.size, 480);

        let msg = b"TEST3>The Quick Brown Fox Jumps Over The Lazy Dog 0123456789";
        let mut data_bits = Vec::new();
        for &byte in msg {
            for i in (0..8).rev() {
                data_bits.push((byte >> i) & 1);
            }
        }
        let enc_data = encode_data_norm(&data_bits);
        assert_eq!(enc_data.len(), 480 + 16);
        let (dec_data, ok) = decode_data_norm(&enc_data, 480);
        assert!(ok);
        assert_eq!(dec_data, data_bits);
    }

    #[test]
    fn conv_data_roundtrip() {
        let data_bits: Vec<u8> = vec![1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 1, 1];
        let enc = encode_data_conv(&data_bits);
        assert_eq!(enc.len(), (data_bits.len() + 16) * 2);
        let (dec, ok) = decode_data_conv(&enc, data_bits.len());
        assert!(ok);
        assert_eq!(dec, data_bits);
    }
}
