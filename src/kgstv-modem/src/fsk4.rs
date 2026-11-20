//! 4-level FSK (4L-FSK) data chunk modulator/demodulator.
//!
//! Simple per-symbol tone correlation (Goertzel/DFT magnitude, as `msk::ToneClock` does)
//! stays at ~80-83% symbol accuracy for 4L-FSK — insufficient (see docs/protocol_spec.md,
//! "Calibration with Known Plaintext"). Root cause: at 1200 baud the 200Hz tone spacing is NOT
//! orthogonal like MSK's 600Hz spacing, so independent/memoryless symbol decisions go wrong
//! due to systematic inter-symbol interference (ISI).
//!
//! **Key observation** (from the modulator formula verified by disassembly: `freq =
//! level×300+1500`, true continuous-phase synthesis — see the `msk.rs` module documentation):
//! the carrier cycles per symbol for the four possible frequencies at 1200 baud:
//!
//! | level | frequency | cycles/symbol | phase contribution (mod 2π) |
//! |---|---|---|---|
//! | 0 | 1200 Hz | 1200/1200 = 1        | 0            |
//! | 1 | 1400 Hz | 1400/1200 = 7/6      | +1×(2π/6)    |
//! | 2 | 1600 Hz | 1600/1200 = 4/3=8/6  | +2×(2π/6)    |
//! | 3 | 1800 Hz | 1800/1200 = 3/2=9/6  | +3×(2π/6)    |
//!
//! So a level-`L` symbol advances the accumulated phase state by EXACTLY `L×60°` (mod 360°) —
//! since the modulation index `h = Δf/baud = 200/1200 = 1/6` is rational, this gives an EXACT (not
//! approximate) 6-state CPM/CPFSK phase trellis (standard Continuous Phase Modulation
//! theory: if `h=p/q` is a reduced fraction, the trellis has `q` states). This module runs
//! Maximum-Likelihood Sequence Detection (Viterbi) over that 6-state trellis to decide the symbol
//! sequence, correctly accounting for phase continuity — instead of treating ISI as an "error
//! source", it models it directly and makes it part of the solution.

use std::f64::consts::PI;

use kgstv_core::constants::FSK4_FREQS;

pub const BAUD: f64 = 1200.0;
const N_STATES: usize = 6;
const N_LEVELS: usize = 4;

/// Converts raw bits (0/1, MSB-first, in pairs) into 4L-FSK symbol levels (0..4).
/// If there is an odd number of bits, a `0` bit is padded at the end (only possible with the
/// NORM+4L-FSK combination; real KG-STV's exact padding rule is unverified but this is the only sensible choice).
pub fn bits_to_levels(bits: &[u8]) -> Vec<u8> {
    let mut levels = Vec::with_capacity(bits.len().div_ceil(2));
    let mut it = bits.iter();
    while let Some(&hi) = it.next() {
        let lo = it.next().copied().unwrap_or(0);
        levels.push((hi << 1) | lo);
    }
    levels
}

/// Converts a level (0..4) into 2 MSB-first bits: `00`→1200Hz(0) ... `11`→1800Hz(3)
/// (verbatim from readme.txt, see docs/protocol_spec.md).
pub fn level_to_bits(level: u8) -> [u8; 2] {
    [(level >> 1) & 1, level & 1]
}

/// Expands a level sequence into an MSB-first bit sequence.
pub fn levels_to_bits(levels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(levels.len() * 2);
    for &l in levels {
        out.extend(level_to_bits(l));
    }
    out
}

/// Converts levels (0..4) into true continuous-phase 4L-FSK audio samples (-1.0..1.0, mono) —
/// the 4-level version of the same formula as `msk::modulate_msk`.
pub fn modulate_fsk4(levels: &[u8], sample_rate: f64) -> Vec<f64> {
    // Exact-time continuous-phase synthesis (see `msk::ToneSynth`): every symbol advances the
    // phase by exactly `level×60°` at any sample rate, keeping the signal on the 6-state trellis.
    let mut synth = crate::msk::ToneSynth::new(sample_rate);
    synth.push_fsk4(levels);
    synth.finish()
}

/// Demodulator that decides a 4L-FSK symbol sequence with Maximum-Likelihood Sequence Detection
/// (6-state CPM phase-trellis Viterbi).
pub struct Fsk4Detector<'a> {
    data: &'a [f64],
    sample_rate: f64,
    tb: f64,
}

impl<'a> Fsk4Detector<'a> {
    pub fn new(data: &'a [f64], sample_rate: f64) -> Self {
        Fsk4Detector { data, sample_rate, tb: sample_rate / BAUD }
    }

    pub fn tb(&self) -> f64 {
        self.tb
    }

    /// The SAME rounding/clamping rule as `msk::ToneClock::window` — so the `t0`/`i` convention
    /// stays exactly aligned (with the MSK bit indices used for header/sync).
    fn window(&self, t0: f64, i: i64) -> (usize, usize) {
        let a = (t0 + i as f64 * self.tb).round();
        let b = (t0 + (i as f64 + 1.0) * self.tb).round();
        let a = a.max(0.0) as usize;
        let mut b = (b.max(0.0) as usize).max(a + 1);
        b = b.min(self.data.len());
        let a = a.min(b);
        (a, b)
    }

    /// Phase-coherent correlation between the real samples and the local reference waveform that
    /// the hypothesis `level` produces over the [a,b) sample range, starting from phase state `state`
    /// (0..6, units of 60°) + global phase offset `phase_offset` — the maximum-likelihood branch metric
    /// under AWGN (since all hypotheses have equal energy there is no need to subtract a constant
    /// energy term; the argmax does not change).
    ///
    /// `t_start` is the symbol's exact (fractional) start position in samples; the window starts at
    /// the rounded sample `a`, so the reference phase is advanced by the sub-sample difference to
    /// keep it aligned with the true trellis phase at `a`.
    fn branch_metric(&self, a: usize, b: usize, t_start: f64, state: usize, level: usize, phase_offset: f64) -> f64 {
        let freq = FSK4_FREQS[level];
        let mut phase = state as f64 * (2.0 * PI / N_STATES as f64)
            + phase_offset
            + 2.0 * PI * freq * (a as f64 - t_start) / self.sample_rate;
        let mut acc = 0.0f64;
        for &x in &self.data[a..b] {
            acc += x * phase.cos();
            phase += 2.0 * PI * freq / self.sample_rate;
        }
        acc
    }

    /// Decides `n_symbols` symbols with Viterbi/MLSD starting from position `t0`+`start_index`
    /// (the sum of `t0`,`start_index` has the same meaning as `msk::ToneClock`'s `(anchor, pos)`
    /// pair — absolute sample position `t0 + start_index*tb`). `phase_offset`: global offset (radians)
    /// scanned to compensate for the unknown absolute carrier phase; the caller should try several
    /// values and pick the best one verified by CRC (see `session.rs`).
    pub fn decode_symbols(&self, t0: f64, start_index: i64, n_symbols: usize, phase_offset: f64) -> Vec<u8> {
        const NEG_INF: f64 = f64::NEG_INFINITY;
        // The REAL phase state at the start of the data chunk is unknown — on the TX side phase may
        // be reset between segments (see `session.rs`), but in REAL recordings the carrier phase
        // probably continues on from the preceding (always MSK) sync/header segment, so the data
        // chunk's initial state can be arbitrary. So give all 6 states an equal (uninformative)
        // prior — pinning a single state (e.g. 0) would systematically decode the WHOLE symbol
        // sequence wrongly if that state is wrong (this was a bug verified with real off-air/controlled
        // recordings; synthetic tests could not catch it because they happened to always start
        // from state 0).
        let mut path_metric = [0.0f64; N_STATES];
        let mut survivor: Vec<[(u8, u8); N_STATES]> = Vec::with_capacity(n_symbols);

        for k in 0..n_symbols {
            let (a, b) = self.window(t0, start_index + k as i64);
            let t_start = t0 + (start_index + k as i64) as f64 * self.tb;
            let mut new_metric = [NEG_INF; N_STATES];
            let mut new_survivor = [(0u8, 0u8); N_STATES];
            for s in 0..N_STATES {
                if path_metric[s] == NEG_INF {
                    continue;
                }
                for level in 0..N_LEVELS {
                    let next = (s + level) % N_STATES;
                    let bm = self.branch_metric(a, b, t_start, s, level, phase_offset);
                    let m = path_metric[s] + bm;
                    if m > new_metric[next] {
                        new_metric[next] = m;
                        new_survivor[next] = (s as u8, level as u8);
                    }
                }
            }
            path_metric = new_metric;
            survivor.push(new_survivor);
        }

        let mut best_state = 0usize;
        let mut best_metric = path_metric[0];
        for s in 1..N_STATES {
            if path_metric[s] > best_metric {
                best_metric = path_metric[s];
                best_state = s;
            }
        }
        let mut levels = vec![0u8; n_symbols];
        let mut state = best_state;
        for k in (0..n_symbols).rev() {
            let (prev, level) = survivor[k][state];
            levels[k] = level;
            state = prev as usize;
        }
        levels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_to_levels_matches_readme_table() {
        // `00`→1200Hz(0) `01`→1400Hz(1) `10`→1600Hz(2) `11`→1800Hz(3)
        assert_eq!(bits_to_levels(&[0, 0, 0, 1, 1, 0, 1, 1]), vec![0, 1, 2, 3]);
        assert_eq!(levels_to_bits(&[0, 1, 2, 3]), vec![0, 0, 0, 1, 1, 0, 1, 1]);
    }

    #[test]
    fn odd_bit_count_padded_with_zero() {
        assert_eq!(bits_to_levels(&[1, 1, 1]), vec![3, 2]); // last symbol: '1','0'(padding) -> 10 -> 2
    }

    #[test]
    fn modulate_demodulate_roundtrip_clean_signal() {
        let sr = 44100.0;
        let levels: Vec<u8> = vec![0, 1, 2, 3, 3, 2, 1, 0, 0, 0, 3, 1, 2, 2, 1, 3, 0, 2, 1, 3];
        let audio = modulate_fsk4(&levels, sr);
        let detector = Fsk4Detector::new(&audio, sr);
        // Since the real absolute phase starts at zero, phase_offset=0 must match exactly.
        let decoded = detector.decode_symbols(0.0, 0, levels.len(), 0.0);
        assert_eq!(decoded, levels);
    }

    /// Regression test: must still decode correctly when the data chunk's initial phase STATE (0..6,
    /// not just a sub-60° fractional offset) is random/unknown. First a "junk" symbol prefix is
    /// modulated to deliberately push the accumulated phase away from state 0, then the actual
    /// payload is decoded ONLY from that point (the index where the junk ends) — simulating
    /// the data chunk in real recordings starting with an arbitrary phase carried over from the
    /// preceding (always MSK) sync/header segment.
    #[test]
    fn decodes_correctly_from_arbitrary_starting_phase_state() {
        let sr = 44100.0;
        let payload: Vec<u8> = vec![2, 0, 1, 3, 3, 1, 0, 2, 2, 2, 0, 3, 1, 1, 3, 0];
        for junk_len in 0..6usize {
            // junk_len symbols (all level=1, i.e. +60°/symbol) shift the accumulated phase by junk_len×60°
            // — the 0..6 range tries all 6 possible initial STATES.
            let junk: Vec<u8> = vec![1u8; junk_len];
            let mut combined = junk.clone();
            combined.extend(&payload);
            let audio = modulate_fsk4(&combined, sr);
            let detector = Fsk4Detector::new(&audio, sr);

            let mut any_exact = false;
            for k in 0..12 {
                let phase_offset = (k as f64 / 12.0) * (2.0 * PI / 6.0);
                let decoded = detector.decode_symbols(0.0, junk_len as i64, payload.len(), phase_offset);
                if decoded == payload {
                    any_exact = true;
                    break;
                }
            }
            assert!(any_exact, "junk_len={junk_len} (initial state={junk_len}): no offset matched exactly");
        }
    }

    #[test]
    fn modulate_demodulate_roundtrip_unknown_phase_offset() {
        // In the real world the absolute phase is unknown: try several offsets, at least one must match
        // exactly (this proves why `session.rs`'s CRC-verified offset scan is necessary).
        let sr = 44100.0;
        let levels: Vec<u8> = vec![1, 3, 0, 2, 2, 0, 3, 1, 1, 1, 2, 3, 0, 0, 2, 1];
        let audio = modulate_fsk4(&levels, sr);
        let detector = Fsk4Detector::new(&audio, sr);
        let mut any_exact = false;
        for k in 0..12 {
            let phase_offset = (k as f64 / 12.0) * (2.0 * PI / 6.0);
            let decoded = detector.decode_symbols(0.0, 0, levels.len(), phase_offset);
            if decoded == levels {
                any_exact = true;
                break;
            }
        }
        assert!(any_exact, "none of the 12 phase-offset scans matched exactly");
    }

    /// Regression test: at sample rates where a symbol is not a whole number of samples (44.1 kHz
    /// = 36.75 samples/symbol, 11.025 kHz = 9.1875), the modulator used to accumulate phase per
    /// sample, which drifted off the CPM trellis over long chunks — large image tiles then failed
    /// CRC even on a clean WAV saved by our own TX.
    #[test]
    fn long_chunk_roundtrip_at_fractional_samples_per_symbol() {
        let mut seed = 0x1234_5678u64;
        let levels: Vec<u8> = (0..4000)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((seed >> 33) % 4) as u8
            })
            .collect();
        for sr in [44100.0, 11025.0, 48000.0] {
            let audio = modulate_fsk4(&levels, sr);
            let detector = Fsk4Detector::new(&audio, sr);
            let decoded = detector.decode_symbols(0.0, 0, levels.len(), 0.0);
            let errors = decoded.iter().zip(&levels).filter(|(a, b)| a != b).count();
            assert_eq!(errors, 0, "sr={sr}: {errors} symbol errors over {} symbols", levels.len());
        }
    }
}
