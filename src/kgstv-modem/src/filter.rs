//! Simple IIR biquad band-pass filter (RBJ cookbook formulas). Used before demodulation to separate
//! KG-STV's tone band (around 1200-1800Hz) from surrounding noise/speech.

use std::f64::consts::PI;

#[derive(Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// Band-pass filter (constant 0dB peak gain), centre frequency `f0` (Hz), quality `q`.
    fn bandpass(sample_rate: f64, f0: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * f0 / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let b0 = alpha;
        let b2 = -alpha;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        Biquad {
            b0: b0 / a0,
            b1: 0.0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Low-pass filter (RBJ cookbook), cutoff frequency `fc` (Hz), quality `q`
    /// (0.707 ≈ Butterworth, maximally flat passband — preferred for a sharp cutoff when
    /// used in cascaded stages).
    fn lowpass(sample_rate: f64, fc: f64, q: f64) -> Self {
        let w0 = 2.0 * PI * fc / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let b1 = 1.0 - cos_w0;
        let b0 = b1 / 2.0;
        let b2 = b0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x0: f64) -> f64 {
        let y0 =
            self.b0 * x0 + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x0;
        self.y2 = self.y1;
        self.y1 = y0;
        y0
    }
}

/// Signal filtered forward+backward (filtfilt style, zero-phase) passing the `lo`..`hi` (Hz) band.
pub fn bandpass_filtfilt(samples: &[f64], sample_rate: f64, lo: f64, hi: f64) -> Vec<f64> {
    let center = (lo + hi) / 2.0;
    let bw = hi - lo;
    let q = center / bw;

    let mut fwd_filter = Biquad::bandpass(sample_rate, center, q);
    let mut fwd: Vec<f64> = samples.iter().map(|&x| fwd_filter.process(x)).collect();

    fwd.reverse();
    let mut bwd_filter = Biquad::bandpass(sample_rate, center, q);
    let mut out: Vec<f64> = fwd.iter().map(|&x| bwd_filter.process(x)).collect();
    out.reverse();
    out
}

/// Signal filtered forward+backward (filtfilt, zero-phase) passing below `cutoff` Hz — for filtering
/// the baseband I/Q components of the FM/FSK discriminator (see `msk::instantaneous_freq`).
/// Uses a two-stage biquad cascade (sharper cutoff than a single stage — important for suppressing
/// leakage from neighbouring symbols and passing only the in-symbol frequency content).
pub fn lowpass_filtfilt(samples: &[f64], sample_rate: f64, cutoff: f64) -> Vec<f64> {
    let q = std::f64::consts::FRAC_1_SQRT_2; // Butterworth (maximally flat passband)
    fn run_stage(x: &[f64], sample_rate: f64, cutoff: f64, q: f64) -> Vec<f64> {
        let mut f1 = Biquad::lowpass(sample_rate, cutoff, q);
        let mut f2 = Biquad::lowpass(sample_rate, cutoff, q);
        x.iter().map(|&v| f2.process(f1.process(v))).collect()
    }
    let mut fwd = run_stage(samples, sample_rate, cutoff, q);
    fwd.reverse();
    let mut out = run_stage(&fwd, sample_rate, cutoff, q);
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_in_band_tone_attenuates_out_of_band() {
        let sr = 8000.0;
        let n = 4000;
        let t: Vec<f64> = (0..n).map(|i| i as f64 / sr).collect();
        let in_band: Vec<f64> = t.iter().map(|&t| (2.0 * PI * 1500.0 * t).sin()).collect();
        let out_of_band: Vec<f64> = t.iter().map(|&t| (2.0 * PI * 200.0 * t).sin()).collect();

        let f_in = bandpass_filtfilt(&in_band, sr, 900.0, 2200.0);
        let f_out = bandpass_filtfilt(&out_of_band, sr, 900.0, 2200.0);

        let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
        // take the middle part to exclude the edge transition region (first/last few hundred samples)
        fn mid(v: &[f64]) -> &[f64] {
            &v[500..v.len() - 500]
        }
        assert!(rms(mid(&f_in)) > 5.0 * rms(mid(&f_out)));
    }

    #[test]
    fn lowpass_passes_low_tone_attenuates_high_tone() {
        let sr = 8000.0;
        let n = 4000;
        let t: Vec<f64> = (0..n).map(|i| i as f64 / sr).collect();
        let low: Vec<f64> = t.iter().map(|&t| (2.0 * PI * 100.0 * t).sin()).collect();
        let high: Vec<f64> = t.iter().map(|&t| (2.0 * PI * 2000.0 * t).sin()).collect();

        let f_low = lowpass_filtfilt(&low, sr, 500.0);
        let f_high = lowpass_filtfilt(&high, sr, 500.0);

        let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
        fn mid(v: &[f64]) -> &[f64] {
            &v[500..v.len() - 500]
        }
        assert!(rms(mid(&f_low)) > 5.0 * rms(mid(&f_high)));
    }
}
