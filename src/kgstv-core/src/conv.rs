//! Rate-1/2, K=7 convolutional code (NASA standard, generator polynomials 171/133 octal).
//! Output order: for each input bit (G133_bit, G171_bit) — verified against a real off-air recording
//! with metric=0 (see docs/protocol_spec.md).
//!
//! Note: the KG-STV header is ALWAYS encoded with this code (54 raw bits -> 108 encoded bits,
//! NO tail/flush bits). The data chunk is encoded with the same code if the header's `c` field is CONV.

const K: usize = 7;
const NSTATES: usize = 1 << (K - 1); // 64
const G1: u8 = 0b1111001; // 171 octal
const G2: u8 = 0b1011011; // 133 octal

fn taps(poly: u8) -> [u8; K] {
    let mut t = [0u8; K];
    for i in 0..K {
        t[i] = (poly >> (K - 1 - i)) & 1;
    }
    t
}

/// `bits` (raw input bits) -> encoded output (of length 2*bits.len()).
/// Output order: for each input bit [G2_output, G1_output] (verified convention).
pub fn encode(bits: &[u8]) -> Vec<u8> {
    let g1 = taps(G1);
    let g2 = taps(G2);
    let mut state: u8 = 0; // most recent previous bit in the LSB
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

/// Viterbi (hard-decision) decode. `rx`: encoded bit sequence (of length 2*nbits,
/// ordered as [o2,o1] pairs). `start_state`: known initial encoder state (usually 0).
/// Returns: (decoded raw bits, total metric/errors, end state).
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
        // This is exactly the 54-bit header decoded with Viterbi at metric=0 from the real
        // off-air recording KG-STV_MSK_text.mp3 (see docs/protocol_spec.md).
        let raw: Vec<u8> = vec![0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,1,0,0,0,0,1,1,1,1,0,0,0,0,0,0,1,1,0,0,1,1,1,0,0,0,0,0,0,1,0];
        assert_eq!(raw.len(), 54);
        let enc = encode(&raw);
        assert_eq!(enc.len(), 108);
        let (dec, metric, _end_state) = viterbi_decode(&enc, 54, 0);
        assert_eq!(metric, 0);
        assert_eq!(dec, raw);
    }
}
