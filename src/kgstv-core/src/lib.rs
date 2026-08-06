//! kgstv-core: KG-STV protokolünün platform-bağımsız çekirdeği.
//!
//! JJ0OBZ'nin orijinal KG-STV yazılımının (Windows-only) protokolünü, orijinal readme.txt'sinin
//! çevirisi ve gerçek off-air ses kayıtları üzerinde bit-seviyesinde ters mühendislikle
//! doğrulanmış hâliyle yeniden üreten çekirdek kütüphane. Bkz. `docs/protocol_spec.md`.
//!
//! Kapsam (bu sürümde): framing (preamble/sync), rate-1/2 K=7 konvolüsyonel kod, 127-bit
//! whitening, CRC-16/AUG-CCITT, 54-bit bilgi chunk'ı (header) — hepsi MSK modu için gerçek
//! kayıtla doğrulandı. 4L-FSK demodülasyonu ve JPEG kuantizasyon ölçeği henüz eksik/açık
//! (bkz. protocol_spec.md, "Hâlâ Belirsiz" bölümleri).

pub mod constants;
pub mod crc;
pub mod conv;
pub mod whiten;
pub mod header;
pub mod frame;

pub use constants::{Command, CodingMode, Modulation};
pub use header::InfoChunk;
