//! Anlık frekans (instantaneous frequency) discriminator: bir CPFSK/FM sinyalinin taşıdığı
//! frekansı örnek-örnek doğrudan tahmin eder — matched-filter/korelasyon tabanlı yöntemlerin
//! aksine (bkz. `msk::ToneClock::soft_metric_at`), sembol içinde DÜZ bir plato ve sembol
//! sınırlarında KESKİN bir geçiş üretir; bu, klasik osiloskop göz diyagramı görünümü
//! (düz üst/alt "raylar" + net kesişim noktaları) için gereken sinyal türüdür — bir matched-
//! filter korelasyonunun ürettiği yumuşatılmış/sinüsvari eğriler DEĞİL.
//!
//! `kgstv.exe`'nin gerçek RX zincirinde kullandığı Hilbert-transform tabanlı FM discriminator
//! ile aynı ailede (disassembly ile doğrulanmış — bkz. docs/protocol_spec.md, "RX zincirinin
//! tam mimarisi").

use std::f64::consts::PI;

use crate::filter::lowpass_filtfilt;

/// `samples`'in `center_freq` (Hz) civarındaki anlık frekansını, örnek başına bir değer olarak
/// (Hz cinsinden mutlak frekans) döndürür.
///
/// `lp_cutoff`: baseband alçak geçiren kesim frekansı — beklenen azami sapmadan (KG-STV için
/// ±300Hz) büyük olmalı ki gerçek frekans içeriği bastırılmasın, ama komşu sembollerin/gürültünün
/// sızıntısını bastıracak kadar dar olmalı. KG-STV'nin 1200 baud'u için ~400-500Hz iyi çalışıyor.
pub fn instantaneous_freq(samples: &[f64], sample_rate: f64, center_freq: f64, lp_cutoff: f64) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    // Baseband'e indir (kompleks karıştırma): x[n] * e^{-j2π·fc·n/fs}
    let mut re = vec![0.0f64; n];
    let mut im = vec![0.0f64; n];
    for i in 0..n {
        let ph = -2.0 * PI * center_freq * i as f64 / sample_rate;
        re[i] = samples[i] * ph.cos();
        im[i] = samples[i] * ph.sin();
    }
    let re_f = lowpass_filtfilt(&re, sample_rate, lp_cutoff);
    let im_f = lowpass_filtfilt(&im, sample_rate, lp_cutoff);

    // Genlik geçidi (magnitude gating): karmaşık baseband |z[n]| küçükken (ör. gerçek kayıtlarda
    // ara sıra oluşan derin sönümlenme/gürültü anları) ardışık-örnek faz farkı SAYISAL OLARAK
    // KARARSIZ hâle gelir — küçük bir gürültü, atan2'yi rastgele bir açıya sıçratıp göz
    // diyagramında merkez frekansta sahte bir yığılma ve uçlarda gürültülü sıçramalar üretir
    // (gerçek kayıtlarla doğrulandı). Ortalama genliğin bir kısmının altına düşen örneklerde
    // son geçerli frekansı korumak bu kararsızlığı büyük ölçüde gideriyor — klasik dijital FM
    // discriminator tasarımlarında standart bir teknik.
    let mags: Vec<f64> = re_f.iter().zip(im_f.iter()).map(|(&r, &i)| (r * r + i * i).sqrt()).collect();
    let mean_mag = mags.iter().sum::<f64>() / mags.len().max(1) as f64;
    let gate = mean_mag * 0.3;

    // Ardışık örnek faz farkı (unwrap gerektirmeyen, sağlam yöntem):
    // freq[n] = fc + fs/(2π) · açı(z[n]·conj(z[n-1]))
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
        // Uzun tek-seviye koşular + her iki uçta bolca dolgu sembolü: `lowpass_filtfilt`
        // (ileri+geri) sinyalin gerçek başı/sonu yakınında geçici (settling) durumda olur —
        // bunu ölçtüğümüz sembollerden uzak tutmak için tamponun her iki ucuna da fazladan
        // sembol ekliyoruz (kısa/dolgusuz bir tamponda ölçüm, geçerli bir demodülatör
        // hatası değil, salt bir kenar-etkisi test artefaktı verirdi).
        let bits: Vec<u8> = vec![0, 0, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1];
        let audio = modulate_msk(&bits, sr);
        let freq = instantaneous_freq(&audio, sr, 1500.0, 500.0);
        let tb = sr / 1200.0;

        // 7. sembolün (0, indeks 6) ortası: 1200Hz civarında olmalı.
        let idx0 = (6.5 * tb) as usize;
        assert!((freq[idx0] - 1200.0).abs() < 20.0, "freq={} (1200 bekleniyor)", freq[idx0]);

        // 11. sembolün (1, indeks 10) ortası: 1800Hz civarında olmalı.
        let idx1 = (10.5 * tb) as usize;
        assert!((freq[idx1] - 1800.0).abs() < 20.0, "freq={} (1800 bekleniyor)", freq[idx1]);
    }
}
