//! CRC-16/AUG-CCITT (poly=0x1021, init=0x1D0F, refin=false, refout=false, xorout=0x0000).
//! Gerçek KG-STV off-air kaydı üzerinde header VE data chunk CRC'leri için bit-mükemmel
//! doğrulandı (bkz. docs/protocol_spec.md).

const POLY: u16 = 0x1021;
const INIT: u16 = 0x1D0F;

/// Bit-serial CRC hesabı. `bits`: MSB-first sıradaki 0/1 dizisi.
pub fn crc16_aug_ccitt(bits: &[u8]) -> u16 {
    let mut reg: u16 = INIT;
    for &bit in bits {
        let top = (reg >> 15) & 1;
        let newtop = top ^ (bit as u16);
        reg <<= 1;
        if newtop != 0 {
            reg ^= POLY;
        }
    }
    reg
}

/// Bayt dizisi üzerinden CRC (MSB-first, byte-hizalı veri için kolaylık fonksiyonu).
pub fn crc16_aug_ccitt_bytes(data: &[u8]) -> u16 {
    let mut bits = Vec::with_capacity(data.len() * 8);
    for &byte in data {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1);
        }
    }
    crc16_aug_ccitt(&bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_from_str(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    #[test]
    fn header_crc_matches_real_capture() {
        // KG-STV_MSK_text.mp3'ten decode edilen gerçek 38-bit header alanı (sys..size) ve CRC'si.
        let hdr38 = bits_from_str("00000000000000000000001010000111100000");
        assert_eq!(hdr38.len(), 38);
        assert_eq!(crc16_aug_ccitt(&hdr38), 0x6702);
    }

    #[test]
    fn data_crc_matches_known_text() {
        let msg = b"TEST3>The Quick Brown Fox Jumps Over The Lazy Dog 0123456789";
        assert_eq!(crc16_aug_ccitt_bytes(msg), 0xA2D1);
    }
}
