//! Info chunk (header): 54 raw bits, field layout translated verbatim from readme.txt and
//! verified against a real off-air recording (see docs/protocol_spec.md).
//!
//! ┌──────┬──────┬───┬───┬──────┬──────┬──────┬──────┬──────┐
//! │ sys  │ com  │ c │ m │  x   │  y   │  sc  │ size │ CRC  │
//! └──────┴──────┴───┴───┴──────┴──────┴──────┴──────┴──────┘
//!  0      4      8   9   10     16     22     26     38   54 bit

use crate::crc::crc16_aug_ccitt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfoChunk {
    pub sys: u8,
    pub com: u8,
    pub c: u8,
    pub m: u8,
    pub x: u8,
    pub y: u8,
    pub sc: u8,
    pub size: u16,
    pub crc: u16,
}

fn bits_to_u32(bits: &[u8]) -> u32 {
    bits.iter().fold(0u32, |acc, &b| (acc << 1) | b as u32)
}

fn u32_to_bits(mut v: u32, width: usize) -> Vec<u8> {
    let mut out = vec![0u8; width];
    for i in (0..width).rev() {
        out[i] = (v & 1) as u8;
        v >>= 1;
    }
    out
}

impl InfoChunk {
    /// Parses the header fields from a 54-bit raw sequence (CRC is not verified; call that separately).
    pub fn parse(bits54: &[u8]) -> Self {
        assert_eq!(bits54.len(), 54);
        InfoChunk {
            sys: bits_to_u32(&bits54[0..4]) as u8,
            com: bits_to_u32(&bits54[4..8]) as u8,
            c: bits54[8],
            m: bits54[9],
            x: bits_to_u32(&bits54[10..16]) as u8,
            y: bits_to_u32(&bits54[16..22]) as u8,
            sc: bits_to_u32(&bits54[22..26]) as u8,
            size: bits_to_u32(&bits54[26..38]) as u16,
            crc: bits_to_u32(&bits54[38..54]) as u16,
        }
    }

    /// Computes the CRC over the sys..size fields (first 38 bits) and compares it with the `crc` field.
    pub fn crc_ok(&self, bits54: &[u8]) -> bool {
        crc16_aug_ccitt(&bits54[..38]) == self.crc
    }

    /// Packs the header fields into a 54-bit raw sequence (CRC is computed automatically).
    pub fn to_bits(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(54);
        out.extend(u32_to_bits(self.sys as u32, 4));
        out.extend(u32_to_bits(self.com as u32, 4));
        out.push(self.c);
        out.push(self.m);
        out.extend(u32_to_bits(self.x as u32, 6));
        out.extend(u32_to_bits(self.y as u32, 6));
        out.extend(u32_to_bits(self.sc as u32, 4));
        out.extend(u32_to_bits(self.size as u32, 12));
        let crc = crc16_aug_ccitt(&out[..38]);
        out.extend(u32_to_bits(crc as u32, 16));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header decoded from the real off-air recording KG-STV_MSK_text.mp3.
    const REAL_HEADER_BITS: &str = "000000000000000000000010100001111000000110011100000010";

    fn bits(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    #[test]
    fn parses_real_capture_correctly() {
        let b = bits(REAL_HEADER_BITS);
        let hdr = InfoChunk::parse(&b);
        assert_eq!(hdr.sys, 0);
        assert_eq!(hdr.com, 0); // TEXT
        assert_eq!(hdr.c, 0);
        assert_eq!(hdr.m, 0);
        assert_eq!(hdr.x, 0);
        assert_eq!(hdr.y, 0);
        assert_eq!(hdr.sc, 10); // cross-checked against kgstv.ini ImageQ=10
        assert_eq!(hdr.size, 480);
        assert!(hdr.crc_ok(&b));
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let hdr = InfoChunk { sys: 0, com: 1, c: 1, m: 0, x: 12, y: 9, sc: 7, size: 375, crc: 0 };
        let packed = hdr.to_bits();
        assert_eq!(packed.len(), 54);
        let reparsed = InfoChunk::parse(&packed);
        assert_eq!(reparsed.sys, hdr.sys);
        assert_eq!(reparsed.com, hdr.com);
        assert_eq!(reparsed.x, hdr.x);
        assert_eq!(reparsed.y, hdr.y);
        assert_eq!(reparsed.size, hdr.size);
        assert!(reparsed.crc_ok(&packed));
    }
}
