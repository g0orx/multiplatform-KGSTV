//! Instantaneous frequency discriminator: directly estimates, sample by sample, the frequency
//! carried by a CPFSK/FM signal — unlike matched-filter/correlation-based methods
//! (see `msk::ToneClock::soft_metric_at`), it produces a FLAT plateau within a symbol and a
//! SHARP transition at symbol boundaries; this is the kind of signal needed for the classic
//! oscilloscope eye-diagram look (flat upper/lower "rails" + clear crossing points) — NOT the
//! smoothed/sinusoidal curves produced by a matched-filter correlation.
//!
//! Same family as the Hilbert-transform-based FM discriminator that `kgstv.exe` uses in its real
//! RX chain (verified by disassembly — see docs/protocol_spec.md, "Full architecture of the
//! RX chain").

use std::f64::consts::PI;

use crate::filter::lowpass_filtfilt;

/// Returns the instantaneous frequency of `samples` around `center_freq` (Hz), as one value
/// per sample (absolute frequency in Hz).
///
/// `lp_cutoff`: baseband low-pass cutoff frequency — must be larger than the expected maximum
/// deviation (±300Hz for KG-STV) so the real frequency content is not suppressed, but narrow
/// enough to suppress leakage from neighbouring symbols/noise. ~400-500Hz works well for KG-STV's 1200 baud.
pub fn instantaneous_freq(samples: &[f64], sample_rate: f64, center_freq: f64, lp_cutoff: f64) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    // Mix down to baseband (complex mixing): x[n] * e^{-j2π·fc·n/fs}
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    for i in 0..n {
        let ph = -2.0 * PI * center_freq * i as f64 / sample_rate;
        re[i] = samples[i] * ph.cos();
        im[i] = samples[i] * ph.sin();
    }
    let re_f = lowpass_filtfilt(&re, sample_rate, lp_cutoff);
    let im_f = lowpass_filtfilt(&im, sample_rate, lp_cutoff);

    // Magnitude gating: when the complex baseband |z[n]| is small (e.g. moments of deep fading/noise
    // that occasionally occur in real recordings) the consecutive-sample phase difference becomes
    // NUMERICALLY UNSTABLE — a little noise makes atan2 jump to a random angle, producing a spurious
    // pile-up at the centre frequency and noisy spikes at the extremes of the eye diagram
    // (verified with real recordings). Holding the last valid frequency for samples that drop below
    // a fraction of the mean magnitude largely removes this instability — a standard technique in
    // classic digital FM discriminator designs.
    let mags: Vec<f64> = re_f.iter().zip(im_f.iter()).map(|(&r, &i)| (r * r + i * i).sqrt()).collect();
    let mean_mag = mags.iter().sum::<f64>() / mags.len().max(1) as f64;
    let gate = mean_mag * 0.3;

    // Consecutive-sample phase difference (robust method, no unwrap needed):
    // freq[n] = fc + fs/(2π) · angle(z[n]·conj(z[n-1]))
    let mut freq = vec![center_freq; n];
    for i in 1..n {
        if mags[i] < gate || mags[i - 1] < gate {
            freq[i] = freq[i - 1];
            continue;
        }
        let dre = re_f[i] * re_f[i - 1] + im_f[i] * im_f[i - 1];
        let dim = im_f[i] * re_f[i - 1] - re_f[i] * im_f[i - 1];
        let dphase = dim.atan2(dre);
        freq[i] = center_freq + dphase * sample_rate / (2.0 * PI);
    }
    freq
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msk::modulate_msk;

    #[test]
    fn tracks_known_msk_frequencies() {
        let sr = 44100.0;
        // Long single-level runs + plenty of padding symbols at both ends: `lowpass_filtfilt`
        // (forward+backward) is in a transient (settling) state near the signal's real start/end —
        // to keep that away from the symbols we measure, we add extra symbols at both ends of
        // the buffer (measuring on a short/unpadded buffer would give a pure edge-effect test
        // artefact, not a genuine demodulator error).
        let bits: Vec<u8> = vec![0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1];
        let audio = modulate_msk(&bits, sr);
        let freq = instantaneous_freq(&audio, sr, 1500.0, 500.0);
        let tb = sr / 1200.0;

        // Middle of the 7th symbol (0, index 6): should be around 1200Hz.
        let idx0 = (6.5 * tb) as usize;
        assert!((freq[idx0] - 1200.0).abs() < 20.0, "freq={} (expected 1200)", freq[idx0]);

        // Middle of the 11th symbol (1, index 10): should be around 1800Hz.
        let idx1 = (10.5 * tb) as usize;
        assert!((freq[idx1] - 1800.0).abs() < 20.0, "freq={} (expected 1800)", freq[idx1]);
    }
}
