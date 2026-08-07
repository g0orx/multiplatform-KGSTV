//! KG-STV'nin 16×16 görüntü tile'ları için JPEG-uyumlu blok codec'i (encode + decode).
//!
//! Gerçek off-air kayıtlar ve kontrollü test görüntüleriyle (düz renk + çizgili, OH2UDS)
//! doğrulanan bulgular:
//! - 6 blok sırası: Y0,Y1,Y2,Y3 (16×16 luma, sol-üst/sağ-üst/sol-alt/sağ-alt), Cr, Cb
//! - **Kritik**: bu 6 bloğun DC değerleri standart JPEG'in aksine BİRBİRİNE ZİNCİRLENMEZ —
//!   her biri kendi başına, predictor=0 ile kodlanır (bkz. docs/protocol_spec.md, "KRİTİK
//!   DÜZELTME" bölümü). Bu codec o bulguyu baştan doğru uyguluyor.
//! - Kuantizasyon: standart IJG "quality" formülü, `sc` header alanı doğrudan quality (1-100).
//! - 4:2:0-eşdeğeri chroma altörnekleme (KG-STV'nin "4:1:1" dediği), tek Cb+Cr 8×8 blok
//!   16×16'lık tüm tile'ı kapsar.
//! - Bitstream marker'sız, byte-hizasız — sadece ham entropy-coded veri + 16-bit CRC (bkz. frame.rs).

use crate::jpeg_tables::*;
use std::collections::HashMap;
use std::sync::OnceLock;

const TILE: usize = 16;
const BLOCK: usize = 8;

// ---------------------------------------------------------------------------------------
// Huffman tabloları (bir kez inşa edilip önbelleklenir)
// ---------------------------------------------------------------------------------------

/// Kanonik JPEG kod ataması (Annex C): (uzunluk, kod) -> değer.
fn build_decode_table(counts: &[u8; 16], values: &[u8]) -> HashMap<(u8, u16), u8> {
    let mut map = HashMap::new();
    let mut code: u16 = 0;
    let mut k = 0usize;
    for len in 1..=16u8 {
        let n = counts[(len - 1) as usize] as usize;
        for _ in 0..n {
            map.insert((len, code), values[k]);
            k += 1;
            code += 1;
        }
        code <<= 1;
    }
    map
}

/// Ters yön: değer -> (kod, uzunluk) — encode için.
fn build_encode_table(counts: &[u8; 16], values: &[u8]) -> HashMap<u8, (u16, u8)> {
    let mut map = HashMap::new();
    let mut code: u16 = 0;
    let mut k = 0usize;
    for len in 1..=16u8 {
        let n = counts[(len - 1) as usize] as usize;
        for _ in 0..n {
            map.insert(values[k], (code, len));
            k += 1;
            code += 1;
        }
        code <<= 1;
    }
    map
}

macro_rules! cached_table {
    ($fn_name:ident, $builder:ident, $counts:expr, $values:expr, $ret:ty) => {
        fn $fn_name() -> &'static $ret {
            static TABLE: OnceLock<$ret> = OnceLock::new();
            TABLE.get_or_init(|| $builder(&$counts, &$values))
        }
    };
}

cached_table!(dc_luma_dec, build_decode_table, DC_LUMA_COUNTS, DC_LUMA_VALUES, HashMap<(u8,u16),u8>);
cached_table!(dc_chroma_dec, build_decode_table, DC_CHROMA_COUNTS, DC_CHROMA_VALUES, HashMap<(u8,u16),u8>);
cached_table!(ac_luma_dec, build_decode_table, AC_LUMA_COUNTS, AC_LUMA_VALUES, HashMap<(u8,u16),u8>);
cached_table!(ac_chroma_dec, build_decode_table, AC_CHROMA_COUNTS, AC_CHROMA_VALUES, HashMap<(u8,u16),u8>);

cached_table!(dc_luma_enc, build_encode_table, DC_LUMA_COUNTS, DC_LUMA_VALUES, HashMap<u8,(u16,u8)>);
cached_table!(dc_chroma_enc, build_encode_table, DC_CHROMA_COUNTS, DC_CHROMA_VALUES, HashMap<u8,(u16,u8)>);
cached_table!(ac_luma_enc, build_encode_table, AC_LUMA_COUNTS, AC_LUMA_VALUES, HashMap<u8,(u16,u8)>);
cached_table!(ac_chroma_enc, build_encode_table, AC_CHROMA_COUNTS, AC_CHROMA_VALUES, HashMap<u8,(u16,u8)>);

// ---------------------------------------------------------------------------------------
// Bit okuma / yazma (kgstv-core genelinde kullanılan Vec<u8> 0/1 bit dizisi konvansiyonu)
// ---------------------------------------------------------------------------------------

struct BitReader<'a> {
    bits: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(bits: &'a [u8]) -> Self {
        BitReader { bits, pos: 0 }
    }
    fn read_bit(&mut self) -> Option<u8> {
        let b = *self.bits.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn read_bits(&mut self, n: u8) -> Option<u32> {
        let mut v: u32 = 0;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()? as u32;
        }
        Some(v)
    }
}

#[derive(Default)]
pub struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    pub fn new() -> Self {
        BitWriter { bits: Vec::new() }
    }
    fn write_bit(&mut self, b: u8) {
        self.bits.push(b & 1);
    }
    fn write_bits(&mut self, value: u32, n: u8) {
        for i in (0..n).rev() {
            self.write_bit(((value >> i) & 1) as u8);
        }
    }
    pub fn into_bits(self) -> Vec<u8> {
        self.bits
    }
}

fn decode_huffman_symbol(br: &mut BitReader, table: &HashMap<(u8, u16), u8>) -> Option<u8> {
    let mut code: u16 = 0;
    for len in 1..=16u8 {
        code = (code << 1) | br.read_bit()? as u16;
        if let Some(&v) = table.get(&(len, code)) {
            return Some(v);
        }
    }
    None
}

fn encode_huffman_symbol(bw: &mut BitWriter, table: &HashMap<u8, (u16, u8)>, symbol: u8) {
    let (code, len) = *table.get(&symbol).expect("huffman tablosunda olmayan sembol");
    bw.write_bits(code as u32, len);
}

/// JPEG EXTEND (T.81 Annex F, F.12): S kategori bitinden imzalı DC/AC değerine.
fn extend(v: u32, s: u8) -> i32 {
    if s == 0 {
        return 0;
    }
    let vt: i32 = 1 << (s - 1);
    let v = v as i32;
    if v < vt {
        v - (1 << s) + 1
    } else {
        v
    }
}

/// EXTEND'in tersi: imzalı bir değeri (kategori, S-bit ham değer) çiftine çevirir.
fn category_of(value: i32) -> (u8, u32) {
    if value == 0 {
        return (0, 0);
    }
    let av = value.unsigned_abs();
    let s = 32 - av.leading_zeros();
    let raw = if value >= 0 {
        value as u32
    } else {
        (value + (1 << s) - 1) as u32
    };
    (s as u8, raw)
}

// ---------------------------------------------------------------------------------------
// DC/AC blok decode + encode (predictor HER ZAMAN 0 — bkz. modül dokümantasyonu)
// ---------------------------------------------------------------------------------------

/// Bir 8×8 bloğun 64 katsayısını (doğal sırada) döndürür. `dc_pred` HER ZAMAN 0 verilmeli.
fn decode_block(
    br: &mut BitReader,
    dc_table: &HashMap<(u8, u16), u8>,
    ac_table: &HashMap<(u8, u16), u8>,
) -> Option<[i32; 64]> {
    let mut coeffs = [0i32; 64];
    let s = decode_huffman_symbol(br, dc_table)?;
    let dc = if s > 0 {
        extend(br.read_bits(s)?, s)
    } else {
        0
    };
    coeffs[0] = dc;
    let mut k = 1usize;
    while k < 64 {
        let rs = decode_huffman_symbol(br, ac_table)?;
        if rs == 0x00 {
            break; // EOB
        }
        if rs == 0xF0 {
            k += 16; // ZRL
            continue;
        }
        let r = (rs >> 4) as usize;
        let sz = rs & 0x0F;
        k += r;
        if k >= 64 {
            break;
        }
        let val = if sz > 0 {
            extend(br.read_bits(sz)?, sz)
        } else {
            0
        };
        coeffs[k] = val;
        k += 1;
    }
    Some(coeffs)
}

/// Bir 8×8 bloğu (zigzag-sıralı 64 katsayı) encode eder.
fn encode_block(
    bw: &mut BitWriter,
    dc_table: &HashMap<u8, (u16, u8)>,
    ac_table: &HashMap<u8, (u16, u8)>,
    zigzag_coeffs: &[i32; 64],
) {
    let (s, raw) = category_of(zigzag_coeffs[0]);
    encode_huffman_symbol(bw, dc_table, s);
    if s > 0 {
        bw.write_bits(raw, s);
    }
    // AC: son sıfır-olmayanın indeksini bul
    let mut last_nz = 0usize;
    for i in (1..64).rev() {
        if zigzag_coeffs[i] != 0 {
            last_nz = i;
            break;
        }
    }
    let mut k = 1usize;
    while k <= last_nz {
        let mut run = 0u8;
        while k <= last_nz && zigzag_coeffs[k] == 0 {
            run += 1;
            k += 1;
            if run == 16 {
                encode_huffman_symbol(bw, ac_table, 0xF0); // ZRL
                run = 0;
            }
        }
        let (s, raw) = category_of(zigzag_coeffs[k]);
        encode_huffman_symbol(bw, ac_table, (run << 4) | s);
        if s > 0 {
            bw.write_bits(raw, s);
        }
        k += 1;
    }
    if last_nz < 63 {
        encode_huffman_symbol(bw, ac_table, 0x00); // EOB
    }
}

// ---------------------------------------------------------------------------------------
// IDCT / DCT (ayrılabilir, 8×8, f64)
// ---------------------------------------------------------------------------------------

fn dct_basis() -> &'static [[f64; 8]; 8] {
    static BASIS: OnceLock<[[f64; 8]; 8]> = OnceLock::new();
    BASIS.get_or_init(|| {
        let mut t = [[0.0f64; 8]; 8];
        for i in 0..8 {
            for k in 0..8 {
                let alpha = if k == 0 { (1.0f64 / 8.0).sqrt() } else { (2.0f64 / 8.0).sqrt() };
                t[i][k] = alpha * ((2 * i + 1) as f64 * k as f64 * std::f64::consts::PI / 16.0).cos();
            }
        }
        t
    })
}

/// spatial = T · F · Tᵀ  (T: dct_basis)
fn idct8(f: &[[f64; 8]; 8]) -> [[f64; 8]; 8] {
    let t = dct_basis();
    let mut tmp = [[0.0f64; 8]; 8];
    for i in 0..8 {
        for l in 0..8 {
            let mut s = 0.0;
            for k in 0..8 {
                s += t[i][k] * f[k][l];
            }
            tmp[i][l] = s;
        }
    }
    let mut out = [[0.0f64; 8]; 8];
    for i in 0..8 {
        for j in 0..8 {
            let mut s = 0.0;
            for l in 0..8 {
                s += tmp[i][l] * t[j][l];
            }
            out[i][j] = s;
        }
    }
    out
}

/// F = Tᵀ · spatial · T  (ileri DCT, encode için)
fn fdct8(spatial: &[[f64; 8]; 8]) -> [[f64; 8]; 8] {
    let t = dct_basis();
    let mut tmp = [[0.0f64; 8]; 8];
    for k in 0..8 {
        for j in 0..8 {
            let mut s = 0.0;
            for i in 0..8 {
                s += t[i][k] * spatial[i][j];
            }
            tmp[k][j] = s;
        }
    }
    let mut out = [[0.0f64; 8]; 8];
    for k in 0..8 {
        for l in 0..8 {
            let mut s = 0.0;
            for j in 0..8 {
                s += tmp[k][j] * t[j][l];
            }
            out[k][l] = s;
        }
    }
    out
}

/// Standart IJG kalite-ölçekleme formülü. `quality`: 1-100 (KG-STV'de header'ın `sc` alanı
/// doğrudan bu değerdir — bkz. docs/protocol_spec.md).
fn scale_qt(base: &[u16; 64], quality: u8) -> [f64; 64] {
    let q = quality.clamp(1, 100) as f64;
    let scale = if q < 50.0 { 5000.0 / q } else { 200.0 - 2.0 * q };
    let mut out = [0.0f64; 64];
    for i in 0..64 {
        let v = ((base[i] as f64 * scale + 50.0) / 100.0).floor();
        out[i] = v.clamp(1.0, 255.0);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Genel API: 16×16 tile decode / encode
// ---------------------------------------------------------------------------------------

/// Bir 16×16 KG-STV görüntü tile'ının decode edilmiş içeriği.
/// `y`: 16×16 luma (0-255 aralığında, IDCT sonrası kırpılmış).
/// `cb`, `cr`: 8×8 chroma (16×16 tile'ın tamamını kapsar, 4:2:0-eşdeğeri).
#[derive(Debug, Clone)]
pub struct Tile {
    pub y: [[f64; TILE]; TILE],
    pub cb: [[f64; BLOCK]; BLOCK],
    pub cr: [[f64; BLOCK]; BLOCK],
}

fn quant_zigzag(spatial: &[[f64; 8]; 8], qt_natural: &[f64; 64]) -> [i32; 64] {
    let freq = fdct8(spatial);
    let mut zz = [0i32; 64];
    for i in 0..64 {
        let pos = ZIGZAG[i];
        let (r, c) = (pos / 8, pos % 8);
        zz[i] = (freq[r][c] / qt_natural[pos]).round() as i32;
    }
    zz
}

/// `bits`: MSB-first 0/1 dizisi, tek bir 16×16 KG-STV görüntü tile'ının ham entropy-coded
/// verisi (CRC hariç — o frame.rs seviyesinde ele alınır). `quality`: header'ın `sc` alanı.
pub fn decode_tile(bits: &[u8], quality: u8) -> Option<Tile> {
    let mut br = BitReader::new(bits);
    let qt_y = scale_qt(&STD_LUMA_QT, quality);
    let qt_c = scale_qt(&STD_CHROMA_QT, quality);

    let mut y_blocks: Vec<[[f64; 8]; 8]> = Vec::with_capacity(4);
    for _ in 0..4 {
        let coeffs = decode_block(&mut br, dc_luma_dec(), ac_luma_dec())?;
        y_blocks.push(dequant_idct_qt(&coeffs, &qt_y));
    }
    let cr_coeffs = decode_block(&mut br, dc_chroma_dec(), ac_chroma_dec())?;
    let cr_block = dequant_idct_qt(&cr_coeffs, &qt_c);
    let cb_coeffs = decode_block(&mut br, dc_chroma_dec(), ac_chroma_dec())?;
    let cb_block = dequant_idct_qt(&cb_coeffs, &qt_c);

    let mut y = [[0.0f64; TILE]; TILE];
    for (bi, blk) in y_blocks.iter().enumerate() {
        let (oy, ox) = (8 * (bi / 2), 8 * (bi % 2)); // 0:TL 1:TR 2:BL 3:BR
        for r in 0..8 {
            for c in 0..8 {
                y[oy + r][ox + c] = (blk[r][c] + 128.0).clamp(0.0, 255.0);
            }
        }
    }
    let mut cb = [[0.0f64; BLOCK]; BLOCK];
    let mut cr = [[0.0f64; BLOCK]; BLOCK];
    for r in 0..8 {
        for c in 0..8 {
            cb[r][c] = (cb_block[r][c] + 128.0).clamp(0.0, 255.0);
            cr[r][c] = (cr_block[r][c] + 128.0).clamp(0.0, 255.0);
        }
    }
    Some(Tile { y, cb, cr })
}

fn dequant_idct_qt(coeffs_zigzag: &[i32; 64], qt: &[f64; 64]) -> [[f64; 8]; 8] {
    let mut natural = [[0.0f64; 8]; 8];
    for i in 0..64 {
        let pos = ZIGZAG[i];
        let (r, c) = (pos / 8, pos % 8);
        natural[r][c] = coeffs_zigzag[i] as f64 * qt[pos];
    }
    idct8(&natural)
}

/// Bir `Tile`'ı KG-STV bitstream'ine encode eder (TX tarafı). `decode_tile` ile round-trip
/// tutarlı olacak şekilde: 6 blok bağımsız (predictor=0), sıra Y0,Y1,Y2,Y3,Cr,Cb.
pub fn encode_tile(tile: &Tile, quality: u8) -> Vec<u8> {
    let qt_y = scale_qt(&STD_LUMA_QT, quality);
    let qt_c = scale_qt(&STD_CHROMA_QT, quality);
    let mut bw = BitWriter::new();

    for bi in 0..4 {
        let (oy, ox) = (8 * (bi / 2), 8 * (bi % 2));
        let mut spatial = [[0.0f64; 8]; 8];
        for r in 0..8 {
            for c in 0..8 {
                spatial[r][c] = tile.y[oy + r][ox + c] - 128.0;
            }
        }
        let zz = quant_zigzag(&spatial, &qt_y);
        encode_block(&mut bw, dc_luma_enc(), ac_luma_enc(), &zz);
    }
    let mut cr_spatial = [[0.0f64; 8]; 8];
    let mut cb_spatial = [[0.0f64; 8]; 8];
    for r in 0..8 {
        for c in 0..8 {
            cr_spatial[r][c] = tile.cr[r][c] - 128.0;
            cb_spatial[r][c] = tile.cb[r][c] - 128.0;
        }
    }
    let zz_cr = quant_zigzag(&cr_spatial, &qt_c);
    encode_block(&mut bw, dc_chroma_enc(), ac_chroma_enc(), &zz_cr);
    let zz_cb = quant_zigzag(&cb_spatial, &qt_c);
    encode_block(&mut bw, dc_chroma_enc(), ac_chroma_enc(), &zz_cb);

    bw.into_bits()
}

/// YCbCr (Y: 16×16, Cb/Cr: 8×8, 4:2:0-eşdeğeri) -> 16×16 RGB (u8, satır-major, [r,g,b]).
pub fn tile_to_rgb(tile: &Tile) -> [[[u8; 3]; TILE]; TILE] {
    let mut out = [[[0u8; 3]; TILE]; TILE];
    for r in 0..TILE {
        for c in 0..TILE {
            let y = tile.y[r][c];
            let cb = tile.cb[r / 2][c / 2];
            let cr = tile.cr[r / 2][c / 2];
            let rr = y + 1.402 * (cr - 128.0);
            let gg = y - 0.344136 * (cb - 128.0) - 0.714136 * (cr - 128.0);
            let bb = y + 1.772 * (cb - 128.0);
            out[r][c] = [
                rr.clamp(0.0, 255.0) as u8,
                gg.clamp(0.0, 255.0) as u8,
                bb.clamp(0.0, 255.0) as u8,
            ];
        }
    }
    out
}

/// 16×16 RGB -> YCbCr `Tile` (encode öncesi). Chroma 8×8'e ortalama ile altörneklenir.
pub fn rgb_to_tile(rgb: &[[[u8; 3]; TILE]; TILE]) -> Tile {
    let mut y = [[0.0f64; TILE]; TILE];
    let mut cb_full = [[0.0f64; TILE]; TILE];
    let mut cr_full = [[0.0f64; TILE]; TILE];
    for r in 0..TILE {
        for c in 0..TILE {
            let [rr, gg, bb] = rgb[r][c].map(|v| v as f64);
            y[r][c] = 0.299 * rr + 0.587 * gg + 0.114 * bb;
            cb_full[r][c] = 128.0 - 0.168736 * rr - 0.331264 * gg + 0.5 * bb;
            cr_full[r][c] = 128.0 + 0.5 * rr - 0.418688 * gg - 0.081312 * bb;
        }
    }
    let mut cb = [[0.0f64; BLOCK]; BLOCK];
    let mut cr = [[0.0f64; BLOCK]; BLOCK];
    for r in 0..BLOCK {
        for c in 0..BLOCK {
            let mut sb = 0.0;
            let mut sr = 0.0;
            for dr in 0..2 {
                for dc in 0..2 {
                    sb += cb_full[2 * r + dr][2 * c + dc];
                    sr += cr_full[2 * r + dr][2 * c + dc];
                }
            }
            cb[r][c] = sb / 4.0;
            cr[r][c] = sr / 4.0;
        }
    }
    Tile { y, cb, cr }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KG-STV_MSK_text.mp3'ten gerçek off-air kayıttan decode edilen, tek bir düz (AC=0)
    /// Y bloğu: DC=-3, quality=10 -> piksel=98.0 (bkz. docs/protocol_spec.md,
    /// "sc → Kuantizasyon Ölçeği — ÇÖZÜLDÜ").
    #[test]
    fn flat_dc_block_matches_real_capture() {
        let qt = scale_qt(&STD_LUMA_QT, 10);
        assert_eq!(qt[0], 80.0); // gerçek kayıtla doğrulanan qt[0,0]
        let mut coeffs = [0i32; 64];
        coeffs[0] = -3;
        let spatial = dequant_idct_qt(&coeffs, &qt);
        let pixel = spatial[0][0] + 128.0;
        assert!((pixel - 98.0).abs() < 1e-9, "pixel={pixel}");
    }

    /// Kritik düzeltme: aynı tile içindeki 4 Y bloğu, hepsi AYNI ham DC diff'e sahip olsa
    /// bile (predictor ZİNCİRLENMEDİĞİ için), decode sonrası hepsi AYNI parlaklığı vermeli —
    /// kontrollü düz-renk test görüntüleriyle (OH2UDS) doğrulandı.
    #[test]
    fn dc_does_not_chain_across_blocks_in_tile() {
        let qt = scale_qt(&STD_LUMA_QT, 10);
        let mut coeffs = [0i32; 64];
        coeffs[0] = -58; // gerçek "duz-kirmizi" testinde her 4 Y bloğunun bağımsız DC'si
        let spatial = dequant_idct_qt(&coeffs, &qt);
        let pixel = (spatial[0][0] + 128.0).clamp(0.0, 255.0);
        // Aynı coeffs her blokta kullanılırsa (chain yok), 4 blok da AYNI pikseli vermeli.
        for _ in 0..4 {
            let s2 = dequant_idct_qt(&coeffs, &qt);
            let p2 = (s2[0][0] + 128.0).clamp(0.0, 255.0);
            assert_eq!(pixel, p2);
        }
    }

    /// UÇTAN UCA GERÇEK VERİ TESTİ: `OH2UDS-duz-kirmizi-mode-msk-code-normal-scale-1.wav`
    /// off-air kaydından, tam demodülasyon+header+CRC zincirinden geçmiş, CRC-doğrulanmış
    /// gerçek 78-bit tile(0,0) verisi. Düz kırmızı bir görüntünün karosu — bu test hem
    /// Huffman decode'un hem de "predictor=0, chain yok" düzeltmesinin GERÇEK sinyal
    /// üzerinde doğru çalıştığını kanıtlıyor (bkz. docs/protocol_spec.md, "KRİTİK DÜZELTME").
    #[test]
    fn decodes_real_captured_solid_red_tile() {
        let bits_str = "111000010110101110000101101011100001011010111000010110101111101100010011001000";
        let bits: Vec<u8> = bits_str.bytes().map(|b| b - b'0').collect();
        assert_eq!(bits.len(), 78);
        let tile = decode_tile(&bits, 10).expect("gerçek kayıttan decode başarısız olmamalı");
        // 4 Y bloğu da (chain olmadığı için) aynı DC=-58 taşıyor -> hepsi Y=0'a kırpılır.
        for row in tile.y.iter() {
            for &v in row.iter() {
                assert_eq!(v, 0.0);
            }
        }
        assert!((tile.cb[0][0] - 74.875).abs() < 1e-9, "cb={}", tile.cb[0][0]);
        assert_eq!(tile.cr[0][0], 255.0);
        let rgb = tile_to_rgb(&tile);
        assert_eq!(rgb[0][0], [178, 0, 0]); // gerçek "duz-kirmizi" testinin ölçülen ortalamasıyla birebir
    }

    #[test]
    fn encode_decode_roundtrip_solid_color() {
        // Düz kırmızı (255,0,0) 16x16 tile — gerçek OH2UDS testiyle aynı ruhta.
        let rgb = [[[255u8, 0, 0]; TILE]; TILE];
        let tile_in = rgb_to_tile(&rgb);
        let bits = encode_tile(&tile_in, 50); // orta kalite, aşırı kırpmayı önlemek için
        let tile_out = decode_tile(&bits, 50).expect("decode başarısız");
        let rgb_out = tile_to_rgb(&tile_out);
        // Merkezi bir piksel makul ölçüde kırmızıya yakın olmalı (JPEG kayıplı olduğu için
        // birebir eşleşme beklenmez, ama kanal ilişkisi doğru olmalı: R >> G,B).
        let [r, g, b] = rgb_out[8][8];
        assert!(r > 150, "r={r}");
        assert!(g < 80, "g={g}");
        assert!(b < 80, "b={b}");
    }

    #[test]
    fn encode_decode_roundtrip_solid_white() {
        let rgb = [[[255u8, 255, 255]; TILE]; TILE];
        let tile_in = rgb_to_tile(&rgb);
        let bits = encode_tile(&tile_in, 50);
        let tile_out = decode_tile(&bits, 50).expect("decode başarısız");
        let rgb_out = tile_to_rgb(&tile_out);
        for row in rgb_out.iter() {
            for px in row.iter() {
                for &ch in px.iter() {
                    assert!(ch > 240, "kanal={ch}, beyaza yakın olmalıydı");
                }
            }
        }
    }
}
