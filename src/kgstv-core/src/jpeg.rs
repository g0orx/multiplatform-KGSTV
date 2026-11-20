//! JPEG-compatible block codec (encode + decode) for KG-STV's 16×16 image tiles.
//!
//! Findings verified with real off-air recordings and controlled test images (solid colour + striped, OH2UDS):
//!
//! - Order of the 6 blocks: Y0,Y1,Y2,Y3 (16×16 luma, top-left/top-right/bottom-left/bottom-right), Cr, Cb
//! - **Critical**: unlike standard JPEG, the DC values of these 6 blocks are NOT CHAINED TO EACH OTHER —
//!   each one is coded on its own with predictor=0 (see docs/protocol_spec.md, the "CRITICAL
//!   CORRECTION" section). This codec applies that finding correctly from the start.
//! - Quantization: the standard IJG quality-scaling formula, but the `sc` header field (4 bits, 0-15)
//!   is NOT a raw "quality" (1-100) — it is an INDEX into the 16 decimal values (0.07…2.0) of the
//!   original sender's "Scale" drop-down list; the scale is computed as a direct multiplier of that value
//!   (see the `scale_qt` documentation — verified against transfer times the user reported from the
//!   real program).
//! - 4:2:0-equivalent chroma subsampling (what KG-STV calls "4:1:1"); a single Cb+Cr 8×8 block
//!   covers the whole 16×16 tile.
//! - The bitstream has no markers and no byte alignment — just raw entropy-coded data + 16-bit CRC (see frame.rs).

use crate::jpeg_tables::*;
use std::collections::HashMap;
use std::sync::OnceLock;

const TILE: usize = 16;
const BLOCK: usize = 8;

// ---------------------------------------------------------------------------------------
// Huffman tables (built once and cached)
// ---------------------------------------------------------------------------------------

/// Canonical JPEG code assignment (Annex C): (length, code) -> value.
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

/// Reverse direction: value -> (code, length) — for encoding.
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
// Bit reading / writing (the Vec<u8> 0/1 bit-array convention used throughout kgstv-core)
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
    let (code, len) = *table.get(&symbol).expect("symbol not in huffman table");
    bw.write_bits(code as u32, len);
}

/// JPEG EXTEND (T.81 Annex F, F.12): from S category bits to a signed DC/AC value.
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

/// Inverse of EXTEND: converts a signed value into a (category, S-bit raw value) pair.
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
// DC/AC block decode + encode (predictor ALWAYS 0 — see module documentation)
// ---------------------------------------------------------------------------------------

/// Returns the 64 coefficients of an 8×8 block (in natural order). `dc_pred` must ALWAYS be 0.
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

/// Encodes an 8×8 block (64 coefficients in zigzag order).
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
    // AC: find the index of the last non-zero coefficient
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
// IDCT / DCT (separable, 8×8, f64)
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

/// F = Tᵀ · spatial · T  (forward DCT, for encoding)
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

/// EXACTLY the same 16 values as the original Windows KGSTV sender's "Scale" drop-down list
/// (reported by the user; see `kgstv-gui::TX_SCALE_LABELS` — the two arrays must be kept
/// in sync). The INDEX (0-15) into this array is written to the header's 4-bit `sc` field;
/// the decimal values set the quantization scale DIRECTLY as a multiplier (see `scale_qt`).
const SC_SCALE_TABLE: [f64; 16] =
    [0.07, 0.08, 0.09, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8, 1.0, 1.20, 1.40, 1.60, 1.80, 2.0];

/// For `sc=10` (Scale="1.0", the original sender's default), the IJG-standard quality scale verified
/// to pixel precision from a real off-air recording (see
/// `flat_dc_block_matches_real_capture`: `quality=10 → scale=5000/10=500`). For all other `sc`:
/// scales the base quantization table according to the `sc` header field (a 0-15 INDEX — looked up in
/// `SC_SCALE_TABLE`, NOT a raw 1-100 IJG "quality").
///
/// **Correction #2 (2026-08-08, verified WITH REAL REFERENCE IMAGES):** The previous
/// correction (`internal_scale = SC_SCALE_TABLE[sc] × 500`, a fixed anchor derived from the IJG
/// "quality=10" formula) got the DIRECTION right but the MAGNITUDE was still wrong — the user
/// reported "image still bad except at sc 2.0" (only the coarsest setting came out acceptable;
/// at EVERY other value the colours/image were corrupted). Using the REAL off-air recordings in
/// `research/sampleaudio/` (`KG-STV_.10_scale.wav` → sc=3 verified,
/// `KG-STV_.50_scale.wav` → sc=7 verified, `KG-STV_MSK_Norm.wav` → sc=10) and the matching
/// pixel-precise reference images taken from the original Windows RECEIVER
/// (`1scale.jpg`, `5scale.png`, `1.0_scale.png` — all three exactly 320×240),
/// a back-calculation was done on many (15+) flat/single-colour (AC≈0) tiles:
/// the real `qval` came out consistently ~5× SMALLER than the OLD formula predicted (80 at sc=10)
/// (~16 at sc=10, ~8 at sc=7, ~2 at sc=3) — and these three points fit a SINGLE, SIMPLE relationship
/// perfectly:
///
/// > **`qval = round(base_table_value × SC_SCALE_TABLE[sc])`** — i.e. the decimal value shown in
/// > "Scale" (0.07…2.0) is itself the DIRECT multiplier of the base (quality=50) quantization
/// > table. There is NO need for an extra intermediate IJG "quality" formula (`5000/quality` etc.) —
/// > the name is already "Scale" (scale/multiplier), not an abstract "quality".
///
/// (The earlier `sc=10 → qt[0,0]=80` claim — `docs/protocol_spec.md`, "RESOLVED" section — had been
/// back-calculated from a single flat block and the source of that measurement could not be
/// independently verified; the 15+ point cross-check against real reference images here is much
/// stronger evidence. The `decodes_real_captured_solid_red_tile` test was updated with this correction.)
fn scale_qt(base: &[u16; 64], sc: u8) -> [f64; 64] {
    let scale = SC_SCALE_TABLE[(sc as usize).min(SC_SCALE_TABLE.len() - 1)];
    let mut out = [0.0f64; 64];
    for i in 0..64 {
        out[i] = (base[i] as f64 * scale).round().clamp(1.0, 255.0);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Public API: 16×16 tile decode / encode
// ---------------------------------------------------------------------------------------

/// Decoded contents of a 16×16 KG-STV image tile.
/// `y`: 16×16 luma (in the 0-255 range, clipped after IDCT).
/// `cb`, `cr`: 8×8 chroma (covers the whole 16×16 tile, 4:2:0-equivalent).
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

/// Diagnostic helper: returns the DC coefficient (raw, BEFORE dequantization) of ALL 6 blocks (Y0..Y3,Cr,Cb) —
/// used for cross-checking against reference images while investigating quantization-scale problems in
/// real recordings (see the "Correction #2" documentation of `scale_qt`); kept in case a similar
/// calibration problem comes up again.
pub fn decode_tile_raw_dc(bits: &[u8]) -> Option<[i32; 6]> {
    let mut br = BitReader::new(bits);
    let mut out = [0i32; 6];
    for o in out.iter_mut().take(4) {
        let coeffs = decode_block(&mut br, dc_luma_dec(), ac_luma_dec())?;
        *o = coeffs[0];
    }
    let cr = decode_block(&mut br, dc_chroma_dec(), ac_chroma_dec())?;
    out[4] = cr[0];
    let cb = decode_block(&mut br, dc_chroma_dec(), ac_chroma_dec())?;
    out[5] = cb[0];
    Some(out)
}

/// `bits`: MSB-first 0/1 sequence, the raw entropy-coded data of a single 16×16 KG-STV image tile
/// (excluding the CRC — that is handled at the frame.rs level). `quality`: the header's `sc` field.
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

/// Encodes a `Tile` into a KG-STV bitstream (TX side). Round-trip consistent with
/// `decode_tile`: 6 independent blocks (predictor=0), order Y0,Y1,Y2,Y3,Cr,Cb.
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

/// YCbCr (Y: 16×16, Cb/Cr: 8×8, 4:2:0-equivalent) -> 16×16 RGB (u8, row-major, [r,g,b]).
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

/// 16×16 RGB -> YCbCr `Tile` (before encoding). Chroma is subsampled to 8×8 by averaging.
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

    /// For sc=10 ("Scale"=1.0) the base table is used DIRECTLY (`qt[0,0]=16×1.0=16`) —
    /// cross-checked on 15+ flat tiles against `research/sampleaudio/1.0_scale.png` (taken from the
    /// original Windows RECEIVER, 320×240, pixel-precise reference) (see the `scale_qt`
    /// documentation, "Correction #2"). DC=-3 -> pixel=122.0.
    #[test]
    fn flat_dc_block_matches_real_capture() {
        let qt = scale_qt(&STD_LUMA_QT, 10);
        assert_eq!(qt[0], 16.0); // qt[0,0] verified against the real reference image
        let mut coeffs = [0i32; 64];
        coeffs[0] = -3;
        let spatial = dequant_idct_qt(&coeffs, &qt);
        let pixel = spatial[0][0] + 128.0;
        assert!((pixel - 122.0).abs() < 1e-9, "pixel={pixel}");
    }

    /// Critical correction: the 4 Y blocks within the same tile, if they all have the SAME raw DC diff
    /// (since the predictor is NOT CHAINED), must all give the SAME brightness after decoding —
    /// verified with controlled solid-colour test images (OH2UDS).
    #[test]
    fn dc_does_not_chain_across_blocks_in_tile() {
        let qt = scale_qt(&STD_LUMA_QT, 10);
        let mut coeffs = [0i32; 64];
        coeffs[0] = -58; // independent DC of each of the 4 Y blocks in the real "duz-kirmizi" (solid red) test
        let spatial = dequant_idct_qt(&coeffs, &qt);
        let pixel = (spatial[0][0] + 128.0).clamp(0.0, 255.0);
        // If the same coeffs are used in every block (no chain), all 4 blocks must give the SAME pixel.
        for _ in 0..4 {
            let s2 = dequant_idct_qt(&coeffs, &qt);
            let p2 = (s2[0][0] + 128.0).clamp(0.0, 255.0);
            assert_eq!(pixel, p2);
        }
    }

    /// END-TO-END REAL DATA TEST: real, CRC-verified 78-bit tile(0,0) data from the off-air recording
    /// `OH2UDS-duz-kirmizi-mode-msk-code-normal-scale-1.wav`, passed through the full
    /// demodulation+header+CRC chain (sc=10, verified from the real recording). A tile of a solid red
    /// image — this test proves that both the Huffman decode and the "predictor=0, no chain"
    /// correction work correctly on a REAL signal (see
    /// docs/protocol_spec.md, "CRITICAL CORRECTION"). Expected values were updated according to the
    /// "Correction #2" formula of `scale_qt` (see that documentation) — the raw bits are the SAME,
    /// only the quantization scale was corrected.
    #[test]
    fn decodes_real_captured_solid_red_tile() {
        let bits_str = "111000010110101110000101101011100001011010111000010110101111101100010011001000";
        let bits: Vec<u8> = bits_str.bytes().map(|b| b - b'0').collect();
        assert_eq!(bits.len(), 78);
        let tile = decode_tile(&bits, 10).expect("decoding the real recording must not fail");
        // All 4 Y blocks carry the same DC=-58 (since there is no chain) -> all clip to the same pixel
        // (corresponding to dark red brightness) — no longer EXACTLY ZERO (that was an artefact of the
        // old, very coarsely scaled formula).
        for row in tile.y.iter() {
            for &v in row.iter() {
                assert!((v - 12.0).abs() < 1e-9, "y={v}");
            }
        }
        assert!((tile.cb[0][0] - 117.375).abs() < 1e-9, "cb={}", tile.cb[0][0]);
        assert!((tile.cr[0][0] - 232.125).abs() < 1e-9, "cr={}", tile.cr[0][0]);
        let rgb = tile_to_rgb(&tile);
        assert_eq!(rgb[0][0], [157, 0, 0]); // dark red — the R≫G,B relationship is preserved
    }

    #[test]
    fn encode_decode_roundtrip_solid_color() {
        // Solid red (255,0,0) 16x16 tile — in the same spirit as the real OH2UDS test.
        // sc=10 ("Scale"=1.0): the real sender's default; since `sc` is now a 0-15 INDEX
        // (see `scale_qt`), the previously used 50 was an invalid value.
        let rgb = [[[255u8, 0, 0]; TILE]; TILE];
        let tile_in = rgb_to_tile(&rgb);
        let bits = encode_tile(&tile_in, 10);
        let tile_out = decode_tile(&bits, 10).expect("decode failed");
        let rgb_out = tile_to_rgb(&tile_out);
        // A central pixel should be reasonably close to red (since JPEG is lossy an exact
        // match is not expected, but the channel relationship must be right: R >> G,B).
        let [r, g, b] = rgb_out[8][8];
        assert!(r > 150, "r={r}");
        assert!(g < 80, "g={g}");
        assert!(b < 80, "b={b}");
    }

    #[test]
    fn encode_decode_roundtrip_solid_white() {
        let rgb = [[[255u8, 255, 255]; TILE]; TILE];
        let tile_in = rgb_to_tile(&rgb);
        let bits = encode_tile(&tile_in, 10); // sc=10 ("Scale"=1.0, default) — see the note above
        let tile_out = decode_tile(&bits, 10).expect("decode failed");
        let rgb_out = tile_to_rgb(&tile_out);
        for row in rgb_out.iter() {
            for px in row.iter() {
                for &ch in px.iter() {
                    assert!(ch > 240, "channel={ch}, should have been close to white");
                }
            }
        }
    }
}
