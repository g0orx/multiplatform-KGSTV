//! MSK modülatör (TX) ve demodülatör (RX, bit-seviyesi).
//!
//! Modülasyon formülü `kgstv.exe` disassembly'sinden kesin olarak doğrulandı (bkz.
//! docs/protocol_spec.md, "Modülatör formülü"): gerçek sürekli-faz (continuous-phase) sentez,
//! `frekans = seviye×300+1500 Hz`, `faz += 2π·f/örnekleme_hızı` her örnekte.
//!
//! Demodülasyon, sembol başına Hanning-pencereli DFT ton-korelasyonu kullanır (Goertzel'e
//! eşdeğer) — MSK'nin 600Hz ton aralığı 1200 baud'da tam ortogonal olduğu için bu basit
//! yöntem gerçek off-air kayıtlarda mükemmel/üretim-kalitesinde sonuç veriyor (bkz.
//! docs/protocol_spec.md, birçok gerçek istasyonla doğrulandı).

use std::f64::consts::PI;

pub const BAUD: f64 = 1200.0;
pub const MSK_FREQS: [f64; 2] = [1200.0, 1800.0];

/// Ham bitleri (0/1) MSK ses örneklerine (-1.0..1.0, mono) çevirir. `sample_rate`: Hz.
pub fn modulate_msk(bits: &[u8], sample_rate: f64) -> Vec<f64> {
    let tb = sample_rate / BAUD;
    let n_samples = ((bits.len() as f64) * tb).round().max(0.0) as usize;
    let mut out = Vec::with_capacity(n_samples);
    let mut phase = 0.0f64;
    for n in 0..n_samples {
        let bit_idx = ((n as f64) / tb).floor() as usize;
        let bit = *bits.get(bit_idx).unwrap_or(&0);
        let freq = MSK_FREQS[bit as usize];
        phase += 2.0 * PI * freq / sample_rate;
        out.push(phase.cos());
    }
    out
}

/// Filtrelenmiş ses örnekleri üzerinde sembol-seviyesi bit kararları veren yardımcı.
/// `anchor`: sembol 0'ın başladığı örnek konumu (kesirli olabilir — ince zamanlama).
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

    /// Belirli bir frekanstaki ton gücü (Hanning-pencereli DFT korelasyonu büyüklüğü).
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

    /// Tek bir MSK bitini (0/1) sembol indeksi `i`'de karar verir.
    pub fn msk_bit(&self, t0: f64, i: i64) -> u8 {
        let p0 = self.tone_power(t0, i, MSK_FREQS[0]);
        let p1 = self.tone_power(t0, i, MSK_FREQS[1]);
        if p1 > p0 {
            1
        } else {
            0
        }
    }

    /// `t0`'dan başlayarak `n` ardışık MSK bitini kararlaştırır.
    pub fn msk_bits(&self, t0: f64, n: i64) -> Vec<u8> {
        (0..n).map(|i| self.msk_bit(t0, i)).collect()
    }

    /// Herhangi bir kesirli örnek konumunda (sembol ızgarasına bağlı olmadan) yumuşak
    /// (soft) MSK karar metriği: `(p1-p0)/(p1+p0)`, yaklaşık [-1,1] aralığında — -1 saf
    /// 1200Hz (bit 0), +1 saf 1800Hz (bit 1). `center`'ı ortalayan `tb` genişliğinde
    /// Hanning-pencereli bir pencere kullanır. `msk_bit`'in aksine sembol sınırlarına
    /// kilitlenmez; göz diyagramı (eye diagram) gibi zamanlama/gürültü görselleştirmeleri
    /// için tasarlandı.
    pub fn soft_metric_at(&self, center: f64) -> f64 {
        let half = self.tb / 2.0;
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
            let m = clk.soft_metric_at(center);
            // Komşu sembollerin farklı frekanslarından sızan enerji (CPFSK'nin sürekli-faz
            // doğası + tam sembole hizalanmamış merkezleme) genliği zayıflatabilir; işaret
            // (yön) yine de doğru olmalı — bu, sert `msk_bit` kararının aksine sürekli bir
            // görselleştirme metriği (bkz. göz diyagramı).
            if bit == 1 {
                assert!(m > 0.02, "bit=1 için metrik pozitif olmalı, m={m}");
            } else {
                assert!(m < -0.02, "bit=0 için metrik negatif olmalı, m={m}");
            }
        }
    }
}
