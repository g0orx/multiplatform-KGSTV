//! MSK modulator (TX) and demodulator (RX, bit level).
//!
//! The modulation formula was conclusively verified from the `kgstv.exe` disassembly (see
//! docs/protocol_spec.md, "Modulator formula"): true continuous-phase synthesis,
//! `frequency = level×300+1500 Hz`, `phase += 2π·f/sample_rate` on every sample.
//!
//! Demodulation uses a per-symbol Hanning-windowed DFT tone correlation (equivalent to
//! Goertzel) — since MSK's 600Hz tone spacing is exactly orthogonal at 1200 baud, this simple
//! method gives perfect/production-quality results on real off-air recordings (see
//! docs/protocol_spec.md, verified with many real stations).

use std::f64::consts::PI;

pub const BAUD: f64 = 1200.0;
pub const MSK_FREQS: [f64; 2] = [1200.0, 1800.0];

/// Continuous-phase tone synthesizer on an exact 1200-baud timeline, shared by the MSK and
/// 4L-FSK modulators and by the session encoder.
///
/// Each 1200-baud slot is one tone (MSK bit or 4L-FSK symbol). Slot `k` covers the continuous time
/// `[k/BAUD, (k+1)/BAUD)`; every output sample `n` is placed at its EXACT time `n/sample_rate` and
/// the phase is evaluated there, so:
/// - the phase advances by exactly `2π·f/BAUD` per slot (the ideal CPM trellis), even when a slot
///   is not a whole number of samples (44.1 kHz = 36.75 samples/slot);
/// - consecutive segments (sync, header, data, ...) pushed into the same synthesizer share one
///   timeline, so there is no per-segment rounding — previously each segment was modulated
///   separately and rounded to whole samples, which slipped the stream against the receiver's
///   `anchor + slot×tb` clock by ~0.25 samples per frame (≈ 2 symbols over a 300-tile image), and
///   per-sample phase accumulation drifted 4L-FSK off its trellis over long chunks. Both made
///   large 4L-FSK image tiles fail CRC even on a perfectly clean WAV from our own TX.
/// - phase is continuous across segment boundaries, as in real KG-STV transmissions.
pub struct ToneSynth {
    sample_rate: f64,
    slot: usize,
    phase_at_slot: f64,
    out: Vec<f64>,
}

impl ToneSynth {
    pub fn new(sample_rate: f64) -> Self {
        ToneSynth { sample_rate, slot: 0, phase_at_slot: 0.0, out: Vec::new() }
    }

    /// Appends one slot (1/BAUD seconds) of a tone at `freq` Hz.
    pub fn push(&mut self, freq: f64) {
        let slot_start = self.slot as f64 / BAUD;
        let slot_end = (self.slot + 1) as f64 / BAUD;
        loop {
            let t = self.out.len() as f64 / self.sample_rate;
            if t >= slot_end {
                break;
            }
            self.out.push((self.phase_at_slot + 2.0 * PI * freq * (t - slot_start)).cos());
        }
        self.phase_at_slot = (self.phase_at_slot + 2.0 * PI * freq / BAUD).rem_euclid(2.0 * PI);
        self.slot += 1;
    }

    /// Appends MSK bits (0 → 1200 Hz, 1 → 1800 Hz).
    pub fn push_msk(&mut self, bits: &[u8]) {
        for &b in bits {
            self.push(MSK_FREQS[(b & 1) as usize]);
        }
    }

    /// Appends 4L-FSK levels (0..4 → 1200/1400/1600/1800 Hz).
    pub fn push_fsk4(&mut self, levels: &[u8]) {
        for &l in levels {
            self.push(kgstv_core::constants::FSK4_FREQS[(l as usize).min(3)]);
        }
    }

    /// Number of samples produced so far.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    pub fn finish(self) -> Vec<f64> {
        self.out
    }
}

/// Converts raw bits (0/1) into MSK audio samples (-1.0..1.0, mono). `sample_rate`: Hz.
pub fn modulate_msk(bits: &[u8], sample_rate: f64) -> Vec<f64> {
    let mut synth = ToneSynth::new(sample_rate);
    synth.push_msk(bits);
    synth.finish()
}

/// Helper that makes symbol-level bit decisions on filtered audio samples.
/// `anchor`: the sample position where symbol 0 starts (may be fractional — fine timing).
pub struct ToneClock<'a> {
    data: &'a [f64],
    sample_rate: f64,
    tb: f64,
}

impl<'a> ToneClock<'a> {
    pub fn new(data: &'a [f64], sample_rate: f64) -> Self {
        ToneClock { data, sample_rate, tb: sample_rate / BAUD }
    }

    pub fn tb(&self) -> f64 {
        self.tb
    }

    fn window(&self, t0: f64, i: i64) -> &[f64] {
        let a = (t0 + i as f64 * self.tb).round();
        let b = (t0 + (i as f64 + 1.0) * self.tb).round();
        let a = a.max(0.0) as usize;
        let mut b = (b.max(0.0) as usize).max(a + 1);
        b = b.min(self.data.len());
        let a = a.min(b);
        &self.data[a..b]
    }

    /// Tone power at a given frequency (magnitude of a Hanning-windowed DFT correlation).
    pub fn tone_power(&self, t0: f64, i: i64, freq: f64) -> f64 {
        let seg = self.window(t0, i);
        let n = seg.len();
        if n == 0 {
            return 0.0;
        }
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        for (k, &x) in seg.iter().enumerate() {
            let w = if n > 3 {
                0.5 - 0.5 * (2.0 * PI * k as f64 / (n as f64 - 1.0)).cos()
            } else {
                1.0
            };
            let ph = -2.0 * PI * freq * k as f64 / self.sample_rate;
            re += x * w * ph.cos();
            im += x * w * ph.sin();
        }
        (re * re + im * im).sqrt()
    }

    /// Decides a single MSK bit (0/1) at symbol index `i`.
    pub fn msk_bit(&self, t0: f64, i: i64) -> u8 {
        let p0 = self.tone_power(t0, i, MSK_FREQS[0]);
        let p1 = self.tone_power(t0, i, MSK_FREQS[1]);
        if p1 > p0 {
            1
        } else {
            0
        }
    }

    /// Decides `n` consecutive MSK bits starting from `t0`.
    pub fn msk_bits(&self, t0: f64, n: i64) -> Vec<u8> {
        (0..n).map(|i| self.msk_bit(t0, i)).collect()
    }

    /// Soft MSK decision metric at any fractional sample position (not tied to the symbol
    /// grid): `(p1-p0)/(p1+p0)`, roughly in the [-1,1] range — -1 is pure
    /// 1200Hz (bit 0), +1 is pure 1800Hz (bit 1). Uses a Hanning-windowed window of width
    /// `2×half_window` centred on `center`. Unlike `msk_bit` it does not lock to symbol
    /// boundaries; designed for timing/noise visualizations such as the eye
    /// diagram.
    ///
    /// `half_window`: window half-width (samples). For behaviour closest to the hard bit decision
    /// (maximum SNR/matched filter) use `tb/2` — but over almost the whole symbol this spills the
    /// window into the neighbouring symbol and squashes the metric towards zero, making the eye
    /// diagram look "flat". For the expected "open in the middle, closed at the edges" look of a
    /// classic eye diagram, a NARROWER window such as `tb/6`–`tb/4` should be used (see `kgstv-gui`).
    pub fn soft_metric_at(&self, center: f64, half_window: f64) -> f64 {
        let half = half_window;
        let a = (center - half).round().max(0.0) as usize;
        let b = ((center + half).round().max(0.0) as usize).min(self.data.len()).max(a);
        if b <= a {
            return 0.0;
        }
        let seg = &self.data[a..b];
        let n = seg.len();
        let (mut re0, mut im0) = (0.0f64, 0.0f64);
        let (mut re1, mut im1) = (0.0f64, 0.0f64);
        for (k, &x) in seg.iter().enumerate() {
            let w = if n > 3 {
                0.5 - 0.5 * (2.0 * PI * k as f64 / (n as f64 - 1.0)).cos()
            } else {
                1.0
            };
            let xw = x * w;
            let ph0 = -2.0 * PI * MSK_FREQS[0] * k as f64 / self.sample_rate;
            re0 += xw * ph0.cos();
            im0 += xw * ph0.sin();
            let ph1 = -2.0 * PI * MSK_FREQS[1] * k as f64 / self.sample_rate;
            re1 += xw * ph1.cos();
            im1 += xw * ph1.sin();
        }
        let p0 = (re0 * re0 + im0 * im0).sqrt();
        let p1 = (re1 * re1 + im1 * im1).sqrt();
        if p0 + p1 < 1e-9 {
            0.0
        } else {
            (p1 - p0) / (p1 + p0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulate_demodulate_roundtrip() {
        let sr = 44100.0;
        let bits: Vec<u8> = vec![0, 1, 1, 0, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1];
        let audio = modulate_msk(&bits, sr);
        let clk = ToneClock::new(&audio, sr);
        let decoded = clk.msk_bits(0.0, bits.len() as i64);
        assert_eq!(decoded, bits);
    }

    #[test]
    fn soft_metric_matches_hard_bit_polarity() {
        let sr = 44100.0;
        let bits: Vec<u8> = vec![0, 1, 0, 1, 1, 0];
        let audio = modulate_msk(&bits, sr);
        let clk = ToneClock::new(&audio, sr);
        for (i, &bit) in bits.iter().enumerate() {
            let center = (i as f64 + 0.5) * clk.tb();
            let m = clk.soft_metric_at(center, clk.tb() / 2.0);
            // Energy leaking from the different frequencies of neighbouring symbols (the continuous-phase
            // nature of CPFSK + centring not exactly aligned to the symbol) can weaken the amplitude; the sign
            // (direction) must still be right — unlike the hard `msk_bit` decision, this is a continuous
            // visualization metric (see eye diagram).
            if bit == 1 {
                assert!(m > 0.02, "metric must be positive for bit=1, m={m}");
            } else {
                assert!(m < -0.02, "metric must be negative for bit=0, m={m}");
            }
        }
    }
}
