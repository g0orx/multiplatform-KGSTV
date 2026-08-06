//! Whitening (beyazlatma): 127-bit M-sequence ile XOR. Her chunk'ın (header, data) başında
//! index sıfırdan yeniden başlar — gerçek kayıt üzerinde doğrulandı (bkz. docs/protocol_spec.md).

use crate::constants::whitening_bits;

/// `bits` üzerine whitening uygular/kaldırır (XOR kendine terstir, aynı fonksiyon
/// hem encode hem decode için kullanılır). Sayaç her çağrıda 0'dan başlar (bir chunk'a karşılık).
pub fn whiten(bits: &[u8]) -> Vec<u8> {
    let w = whitening_bits();
    bits.iter().enumerate().map(|(i, &b)| b ^ w[i % w.len()]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dewhitening_all_zero_input_reproduces_whitening_sequence() {
        let zeros = vec![0u8; 64];
        let out = whiten(&zeros);
        assert_eq!(out, whitening_bits()[..64]);
    }

    #[test]
    fn whiten_is_self_inverse() {
        let data: Vec<u8> = (0..300).map(|i| (i % 3 == 0) as u8).collect();
        let w1 = whiten(&data);
        let w2 = whiten(&w1);
        assert_eq!(w2, data);
    }
}
