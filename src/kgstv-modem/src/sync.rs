//! Preamble/senkron kelimesi arama: bir kayıtta KG-STV çerçevesinin tam olarak nerede
//! başladığını (örnek-hassasiyetinde "anchor") bulur.

use crate::msk::ToneClock;
use kgstv_core::constants::{sync_word_bits, PREAMBLE_LEN};

pub const SYNC_LEN: usize = 63;

/// `t0` anchor'ında, preamble'ın hemen ardından beklenen 63-bit senkron kelimesiyle olan
/// bit-hata sayısı (0 = tam eşleşme).
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

/// Kaba (tam örnek adımlarıyla) + ince (kesirli) iki aşamalı arama. Temiz/kısa kayıtlar için
/// yeterli — uzun/gürültülü gerçek kayıtlarda tüm dosyayı tarayan vektörize bir arama gerekir
/// (Python prototipindeki `decode_session.SyncLocator` böyle bir yaklaşım kullanıyor).
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
    let (_, base) = best.expect("coarse_range boş olamaz");

    // Eşitlik (tie) durumunda kaba arama tarafından bulunan `base`'e en yakın konumu tercih et:
    // pencere fonksiyonu negatif örnek konumlarını 0'a "clamp" ettiği için (bkz. `window`),
    // sadece en küçük hatayı almak, alakasız derecede uzak/negatif t0 değerleriyle sahte
    // eşitliklere yol açabilir.
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
        // biraz da header benzeri rastgele bit ekleyelim (aramanın gerçek anchor'ı bulduğunu
        // teyit etmek için preamble+sync sonrası veri de olsun)
        bits.extend(vec![0u8; 108]);
        let audio = modulate_msk(&bits, sr);
        let clk = ToneClock::new(&audio, sr);
        let (anchor, err) = find_sync(&clk, 0..500, 3.0, 0.05);
        assert_eq!(err, 0);
        assert!((anchor - 0.0).abs() < 1.0, "anchor={anchor}");
    }
}
