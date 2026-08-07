//! 4-seviyeli FSK (4L-FSK) veri chunk'ı modülatörü/demodülatörü.
//!
//! Basit sembol-bazlı ton-korelasyon (Goertzel/DFT büyüklüğü, `msk::ToneClock`'un yaptığı gibi)
//! 4L-FSK için ~%80-83 sembol doğruluğunda kalıyor — yetersiz (bkz. docs/protocol_spec.md,
//! "Bilinen-Düz-Metinle Kalibrasyon"). Kök neden: 200Hz ton aralığı 1200 baud'da MSK'nin 600Hz
//! aralığı gibi ortogonal DEĞİL, bu yüzden bağımsız/hafızasız sembol kararları sistematik
//! blok-arası girişim (ISI) yüzünden hatalı oluyor.
//!
//! **Anahtar gözlem** (disassembly ile doğrulanmış modülatör formülünden: `freq =
//! seviye×300+1500`, gerçek sürekli-faz sentez — bkz. `msk.rs` modül dokümantasyonu):
//! 1200 baud'da dört olası frekansın sembol başına taşıdığı taşıyıcı çevrimi:
//!
//! | seviye | frekans | çevrim/sembol | faz katkısı (mod 2π) |
//! |---|---|---|---|
//! | 0 | 1200 Hz | 1200/1200 = 1        | 0            |
//! | 1 | 1400 Hz | 1400/1200 = 7/6      | +1×(2π/6)    |
//! | 2 | 1600 Hz | 1600/1200 = 4/3=8/6  | +2×(2π/6)    |
//! | 3 | 1800 Hz | 1800/1200 = 3/2=9/6  | +3×(2π/6)    |
//!
//! Yani seviye `L` sembolü, biriken faz durumunu TAM OLARAK `L×60°` ilerletir (mod 360°) —
//! modülasyon indeksi `h = Δf/baud = 200/1200 = 1/6` rasyonel olduğu için bu KESİN (yaklaşık
//! değil) bir 6 durumlu CPM/CPFSK faz-trellis'i verir (standart Continuous Phase Modulation
//! teorisi: `h=p/q` sadeleştirilmiş kesirse, trellis `q` durumludur). Bu modül, bu 6 durumlu
//! trellis üzerinde Maximum-Likelihood Sequence Detection (Viterbi) çalıştırarak sembol
//! dizisini, faz sürekliliğini doğru hesaba katarak kararlaştırır — ISI'yi bir "hata kaynağı"
//! olarak görmek yerine, doğrudan modelleyip çözüme dahil eder.

use std::f64::consts::PI;

use kgstv_core::constants::FSK4_FREQS;

pub const BAUD: f64 = 1200.0;
const N_STATES: usize = 6;
const N_LEVELS: usize = 4;

/// Ham bitleri (0/1, MSB-first çift çift) 4L-FSK sembol seviyelerine (0..4) çevirir.
/// Tek sayıda bit varsa sonda bir `0` bitiyle doldurulur (yalnızca NORM+4L-FSK kombinasyonunda
/// olası bir durum; gerçek KG-STV'nin tam dolgu kuralı doğrulanmadı ama bu tek makul seçenek).
pub fn bits_to_levels(bits: &[u8]) -> Vec<u8> {
    let mut levels = Vec::with_capacity(bits.len().div_ceil(2));
    let mut it = bits.iter();
    while let Some(&hi) = it.next() {
        let lo = it.next().copied().unwrap_or(0);
        levels.push((hi << 1) | lo);
    }
    levels
}

/// Bir seviyeyi (0..4) MSB-first 2 bite çevirir: `00`→1200Hz(0) ... `11`→1800Hz(3)
/// (readme.txt'den birebir, bkz. docs/protocol_spec.md).
pub fn level_to_bits(level: u8) -> [u8; 2] {
    [(level >> 1) & 1, level & 1]
}

/// Seviye dizisini MSB-first bit dizisine açar.
pub fn levels_to_bits(levels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(levels.len() * 2);
    for &l in levels {
        out.extend(level_to_bits(l));
    }
    out
}

/// Seviyeleri (0..4) gerçek sürekli-faz 4L-FSK ses örneklerine (-1.0..1.0, mono) çevirir —
/// `msk::modulate_msk` ile aynı formülün 4-seviyeli hâli.
pub fn modulate_fsk4(levels: &[u8], sample_rate: f64) -> Vec<f64> {
    let tb = sample_rate / BAUD;
    let n_samples = ((levels.len() as f64) * tb).round().max(0.0) as usize;
    let mut out = Vec::with_capacity(n_samples);
    let mut phase = 0.0f64;
    for n in 0..n_samples {
        let sym_idx = ((n as f64) / tb).floor() as usize;
        let level = *levels.get(sym_idx).unwrap_or(&0) as usize;
        let freq = FSK4_FREQS[level.min(3)];
        phase += 2.0 * PI * freq / sample_rate;
        out.push(phase.cos());
    }
    out
}

/// 4L-FSK sembol dizisini Maximum-Likelihood Sequence Detection (6 durumlu CPM faz-trellis
/// Viterbi) ile kararlaştıran demodülatör.
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

    /// `msk::ToneClock::window` ile AYNI yuvarlama/sınırlama kuralı — `t0`/`i` konvansiyonu
    /// (header/senkron için kullanılan MSK bit-indeksleriyle) birebir hizalı kalsın diye.
    fn window(&self, t0: f64, i: i64) -> (usize, usize) {
        let a = (t0 + i as f64 * self.tb).round();
        let b = (t0 + (i as f64 + 1.0) * self.tb).round();
        let a = a.max(0.0) as usize;
        let mut b = (b.max(0.0) as usize).max(a + 1);
        b = b.min(self.data.len());
        let a = a.min(b);
        (a, b)
    }

    /// Faz durumu `state` (0..6, birim 60°) + global faz ofseti `phase_offset`'ten başlayarak,
    /// seviye `level` hipotezinin [a,b) örnek aralığında ürettiği yerel referans dalga
    /// biçimiyle gerçek örnekler arasındaki faz-uyumlu (coherent) korelasyon — AWGN altında
    /// maksimum-olabilirlik dal metriği (tüm hipotezler eşit enerjili olduğundan sabit bir
    /// enerji terimi çıkarmaya gerek yok, argmax değişmez).
    fn branch_metric(&self, a: usize, b: usize, state: usize, level: usize, phase_offset: f64) -> f64 {
        let freq = FSK4_FREQS[level];
        let mut phase = state as f64 * (2.0 * PI / N_STATES as f64) + phase_offset;
        let mut acc = 0.0f64;
        for &x in &self.data[a..b] {
            acc += x * phase.cos();
            phase += 2.0 * PI * freq / self.sample_rate;
        }
        acc
    }

    /// `t0`+`start_index` konumundan başlayarak `n_symbols` sembolü Viterbi/MLSD ile
    /// kararlaştırır (`t0`,`start_index` toplamı `msk::ToneClock`'un `(anchor, pos)` çiftiyle
    /// aynı anlamda — mutlak örnek konumu `t0 + start_index*tb`). `phase_offset`: bilinmeyen
    /// mutlak taşıyıcı fazını telafi etmek için taranan global ofset (radyan); çağıran birkaç
    /// değer deneyip CRC ile doğrulayan en iyisini seçmelidir (bkz. `session.rs`).
    pub fn decode_symbols(&self, t0: f64, start_index: i64, n_symbols: usize, phase_offset: f64) -> Vec<u8> {
        const NEG_INF: f64 = f64::NEG_INFINITY;
        // Veri chunk'ının başındaki GERÇEK faz durumu bilinmiyor — TX tarafında segmentler
        // arası faz sıfırlanabilir (bkz. `session.rs`), ama GERÇEK kayıtlarda taşıyıcı fazı
        // muhtemelen önceki (her zaman MSK olan) senkron/header segmentinden sürekli devam
        // ediyor, yani veri chunk'ının başlangıç durumu keyfi olabilir. Bu yüzden 6 durumun
        // hepsine eşit (bilgisiz) bir önsel ver — tek bir durumu (ör. 0) sabitlemek, o durum
        // yanlışsa TÜM sembol dizisini sistematik olarak yanlış çözer (gerçek off-air/kontrollü
        // kayıtlarla doğrulanan bir hataydı; sentetik testler kazara hep durum 0'dan başladığı
        // için bunu yakalayamıyordu).
        let mut path_metric = [0.0f64; N_STATES];
        let mut survivor: Vec<[(u8, u8); N_STATES]> = Vec::with_capacity(n_symbols);

        for k in 0..n_symbols {
            let (a, b) = self.window(t0, start_index + k as i64);
            let mut new_metric = [NEG_INF; N_STATES];
            let mut new_survivor = [(0u8, 0u8); N_STATES];
            for s in 0..N_STATES {
                if path_metric[s] == NEG_INF {
                    continue;
                }
                for level in 0..N_LEVELS {
                    let next = (s + level) % N_STATES;
                    let bm = self.branch_metric(a, b, s, level, phase_offset);
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
        assert_eq!(bits_to_levels(&[1, 1, 1]), vec![3, 2]); // son sembol: '1','0'(dolgu) -> 10 -> 2
    }

    #[test]
    fn modulate_demodulate_roundtrip_clean_signal() {
        let sr = 44100.0;
        let levels: Vec<u8> = vec![0, 1, 2, 3, 3, 2, 1, 0, 0, 0, 3, 1, 2, 2, 1, 3, 0, 2, 1, 3];
        let audio = modulate_fsk4(&levels, sr);
        let detector = Fsk4Detector::new(&audio, sr);
        // Gerçek mutlak faz sıfırdan başladığı için phase_offset=0 tam eşleşmeli.
        let decoded = detector.decode_symbols(0.0, 0, levels.len(), 0.0);
        assert_eq!(decoded, levels);
    }

    /// Regresyon testi: veri chunk'ının başlangıç faz DURUMU (0..6, sadece sub-60° kesirli
    /// ofset değil) rastgele/bilinmeyen olduğunda hâlâ doğru çözülmeli. Önce "junk" bir
    /// sembol öneki modüle edilip biriken faz kasıtlı olarak durum 0'dan uzaklaştırılıyor,
    /// sonra asıl payload SADECE o noktadan itibaren (junk'ın bittiği index'ten) decode
    /// ediliyor — gerçek kayıtlarda veri chunk'ının, kendinden önceki (her zaman MSK olan)
    /// senkron/header segmentinden gelen keyfi bir fazla başlamasının simülasyonu.
    #[test]
    fn decodes_correctly_from_arbitrary_starting_phase_state() {
        let sr = 44100.0;
        let payload: Vec<u8> = vec![2, 0, 1, 3, 3, 1, 0, 2, 2, 2, 0, 3, 1, 1, 3, 0];
        for junk_len in 0..6usize {
            // junk_len sembolü (hepsi seviye=1, yani +60°/sembol) biriken fazı junk_len×60°
            // kaydırır — 0..6 aralığı 6 olası başlangıç DURUMUNUN hepsini dener.
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
            assert!(any_exact, "junk_len={junk_len} (başlangıç durumu={junk_len}): hiçbir ofset tam eşleşmedi");
        }
    }

    #[test]
    fn modulate_demodulate_roundtrip_unknown_phase_offset() {
        // Gerçek dünyada mutlak faz bilinmez: birkaç ofset dene, en azından biri tam eşleşmeli
        // (bu, `session.rs`'in CRC-doğrulamalı ofset taramasının neden gerekli olduğunu kanıtlar).
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
        assert!(any_exact, "12 faz ofseti taramasında hiçbiri tam eşleşmedi");
    }
}
