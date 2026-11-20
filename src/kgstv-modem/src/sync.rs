//! Preamble/sync-word search: finds exactly where a KG-STV frame starts in a recording
//! (a sample-precise "anchor").

use crate::msk::ToneClock;
use kgstv_core::constants::{sync_word_bits, PREAMBLE_LEN};

pub const SYNC_LEN: usize = 63;

/// Number of bit errors against the 63-bit sync word expected right after the preamble at
/// anchor `t0` (0 = exact match).
pub fn sync_err(clk: &ToneClock, t0: f64) -> u32 {
    let sw = sync_word_bits();
    let mut err = 0u32;
    for i in 0..SYNC_LEN as i64 {
        let bit = clk.msk_bit(t0, PREAMBLE_LEN as i64 + i);
        if bit != sw[i as usize] {
            err += 1;
        }
    }
    err
}

/// Two-stage coarse (whole-sample steps) + fine (fractional) search. Sufficient for clean/short
/// recordings — long/noisy real recordings need a vectorized search that scans the whole file
/// (`decode_session.SyncLocator` in the Python prototype uses such an approach).
pub fn find_sync(
    clk: &ToneClock,
    coarse_range: impl Iterator<Item = i64>,
    fine_span: f64,
    fine_step: f64,
) -> (f64, u32) {
    let mut best: Option<(u32, i64)> = None;
    for t0 in coarse_range {
        let e = sync_err(clk, t0 as f64);
        if best.is_none_or(|(be, _)| e < be) {
            best = Some((e, t0));
        }
        if e == 0 {
            break;
        }
    }
    let (_, base) = best.expect("coarse_range cannot be empty");

    // On a tie, prefer the position closest to the `base` found by the coarse search:
    // since the window function "clamps" negative sample positions to 0 (see `window`),
    // simply taking the smallest error can lead to spurious ties with irrelevantly
    // distant/negative t0 values.
    let mut best2: Option<(u32, f64)> = None;
    let steps = ((2.0 * fine_span) / fine_step).round() as i64;
    for s in 0..=steps {
        let t = base as f64 - fine_span + s as f64 * fine_step;
        let e = sync_err(clk, t);
        let better = match best2 {
            None => true,
            Some((be, bt)) => {
                e < be || (e == be && (t - base as f64).abs() < (bt - base as f64).abs())
            }
        };
        if better {
            best2 = Some((e, t));
        }
    }
    let (e, t) = best2.unwrap();
    (t, e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msk::modulate_msk;
    use kgstv_core::frame;

    #[test]
    fn finds_sync_in_synthetic_signal() {
        let sr = 44100.0;
        let mut bits = frame::preamble_bits();
        bits.extend(frame::sync_bits());
        // add some header-like random bits too (so there is data after preamble+sync,
        // to confirm the search finds the real anchor)
        bits.extend(vec![0u8; 108]);
        let audio = modulate_msk(&bits, sr);
        let clk = ToneClock::new(&audio, sr);
        let (anchor, err) = find_sync(&clk, 0..500, 3.0, 0.05);
        assert_eq!(err, 0);
        assert!((anchor - 0.0).abs() < 1.0, "anchor={anchor}");
    }
}
