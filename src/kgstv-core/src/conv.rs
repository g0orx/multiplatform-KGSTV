//! Rate-1/2, K=7 konvolüsyonel kod (NASA standart, üreteç polinomları 171/133 oktal).
//! Çıkış sırası: her girdi biti için (G133_biti, G171_biti) — gerçek off-air kayıt üzerinde
//! metric=0 ile doğrulandı (bkz. docs/protocol_spec.md).
//!
//! Not: KG-STV header'ı HER ZAMAN bu kodla kodlanır (54 ham bit -> 108 kodlanmış bit,
//! tail/flush biti YOK). Veri chunk'ı, header'ın `c` alanı CONV ise aynı kodla kodlanır.

const K: usize = 7;
const NSTATES: usize = 1 << (K - 1); // 64
const G1: u8 = 0b1111001; // 171 oktal
const G2: u8 = 0b1011011; // 133 oktal

fn taps(poly: u8) -> [u8; K] {
    let mut t = [0u8; K];
    for i in 0..K {
        t[i] = (poly >> (K - 1 - i)) & 1;
    }
    t
}

/// `bits` (ham girdi bitleri) -> kodlanmış çıktı (2*bits.len() uzunluğunda).
/// Çıkış sırası: her girdi biti için [G2_output, G1_output] (doğrulanmış konvansiyon).
pub fn encode(bits: &[u8]) -> Vec<u8> {
    let g1 = taps(G1);
    let g2 = taps(G2);
    let mut state: u8 = 0; // en yeni önceki bit LSB'de
    let mut out = Vec::with_capacity(bits.len() * 2);
    for &bit in bits {
        let mut reg = [0u8; K];
        reg[0] = bit;
        for i in 0..(K - 1) {
            reg[i + 1] = (state >> i) & 1;
        }
        let o1 = reg.iter().zip(g1.iter()).map(|(a, b)| a & b).fold(0, |acc, x| acc ^ x);
        let o2 = reg.iter().zip(g2.iter()).map(|(a, b)| a & b).fold(0, |acc, x| acc ^ x);
        out.push(o2);
        out.push(o1);
        state = ((state << 1) | bit) & (NSTATES as u8 - 1);
    }
    out
}

/// Viterbi (hard-decision) decode. `rx`: kodlanmış bit dizisi (2*nbits uzunluğunda,
/// [o2,o1] çift sıralı). `start_state`: bilinen başlangıç encoder durumu (genelde 0).
/// Dönüş: (kod çözülmüş ham bitler, toplam metrik/hata, bitiş durumu).
pub fn viterbi_decode(rx: &[u8], nbits: usize, start_state: usize) -> (Vec<u8>, u32, usize) {
    let g1 = taps(G1);
    let g2 = taps(G2);
    const INF: u32 = u32::MAX / 2;

    let mut pm = vec![INF; NSTATES];
    pm[start_state] = 0;
    let mut prev = vec![vec![usize::MAX; NSTATES]; nbits + 1];
    let mut inp = vec![vec![0u8; NSTATES]; nbits + 1];
    let mut pm_hist = vec![pm.clone()];

    for t in 0..nbits {
        let mut npm = vec![INF; NSTATES];
        let r1 = rx[2 * t + 1]; // o1
        let r2 = rx[2 * t]; // o2
        for s in 0..NSTATES {
            if pm_hist[t][s] >= INF {
                continue;
            }
            for &bit in &[0u8, 1u8] {
                let mut reg = [0u8; K];
                reg[0] = bit;
                for i in 0..(K - 1) {
                    reg[i + 1] = ((s >> i) & 1) as u8;
                }
                let o1 = reg.iter().zip(g1.iter()).map(|(a, b)| a & b).fold(0u8, |acc, x| acc ^ x);
                let o2 = reg.iter().zip(g2.iter()).map(|(a, b)| a & b).fold(0u8, |acc, x| acc ^ x);
                let ns = ((s << 1) | bit as usize) & (NSTATES - 1);
                let cost = ((o1 != r1) as u32) + ((o2 != r2) as u32);
                let metric = pm_hist[t][s] + cost;
                if metric < npm[ns] {
                    npm[ns] = metric;
                    prev[t + 1][ns] = s;
                    inp[t + 1][ns] = bit;
                }
            }
        }
        pm_hist.push(npm);
    }

    let last = &pm_hist[nbits];
    let (end_state, &best_metric) = last
        .iter()
        .enumerate()
        .min_by_key(|(_, &m)| m)
        .map(|(i, m)| (i, m))
        .unwrap();

    let mut bits = vec![0u8; nbits];
    let mut s = end_state;
    for t in (1..=nbits).rev() {
        bits[t - 1] = inp[t][s];
        s = prev[t][s];
    }
    (bits, best_metric, end_state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip_with_real_header_bits() {
        // Bu tam olarak KG-STV_MSK_text.mp3'ten gerçek off-air kayıttan Viterbi ile
        // metric=0 çözülen 54-bit header (bkz. docs/protocol_spec.md).
        let raw: Vec<u8> = vec![0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,1,0,0,0,0,1,1,1,1,0,0,0,0,0,0,1,1,0,0,1,1,1,0,0,0,0,0,0,1,0];
        assert_eq!(raw.len(), 54);
        let enc = encode(&raw);
        assert_eq!(enc.len(), 108);
        let (dec, metric, _end_state) = viterbi_decode(&enc, 54, 0);
        assert_eq!(metric, 0);
        assert_eq!(dec, raw);
    }
}
