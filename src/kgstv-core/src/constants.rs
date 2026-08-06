//! KG-STV protokolünün sabitleri.
//! Kaynak: docs/protocol_spec.md (JJ0OBZ'nin orijinal readme.txt'sinden çevrildi,
//! gerçek off-air kayıtlar üzerinde bit-seviyesinde doğrulandı).

/// Sembol hızı (baud). MSK ve 4L-FSK için aynı.
pub const BAUD: f64 = 1200.0;

/// MSK frekansları: [bit0 (space), bit1 (mark)]
pub const MSK_FREQS: [f64; 2] = [1200.0, 1800.0];

/// 4L-FSK frekansları: seviye 0..3 -> Hz. Seviye = 2*bit_yüksek + bit_alçak.
/// '00'=1200Hz '01'=1400Hz '10'=1600Hz '11'=1800Hz
pub const FSK4_FREQS: [f64; 4] = [1200.0, 1400.0, 1600.0, 1800.0];

/// Preamble: 256 bit, "01" tekrarı (senkronizasyon öncesi, clock recovery için).
pub const PREAMBLE_LEN: usize = 256;

/// 63-bit senkron kelimesi (M-sequence), readme.txt'den birebir.
pub const SYNC_WORD: &str = "000011100001001000110110010110101110111100110001010100111111010";

/// 127-bit whitening (beyazlatma) M-sequence kodu. Her chunk'ın (header, data) başında
/// index sıfırdan yeniden başlar (global/sürekli sayaç DEĞİL) — gerçek kayıt üzerinde doğrulandı.
pub const WHITENING: &str = concat!(
    "1110110011000100100111001111100100000100011010101001101101001010",
    "000101100001100101111111010110111011110001110100010101110000001"
);

/// Bilgi chunk'ı (header) ham uzunluğu, bit.
pub const HEADER_RAW_BITS: usize = 54;

/// Header her zaman rate-1/2 K=7 konvolüsyonel kodla kodlanır (veri chunk'ının kendi
/// c/m ayarından bağımsız) -> 108 kodlanmış bit.
pub const HEADER_ENC_BITS: usize = HEADER_RAW_BITS * 2;

/// CRC alan genişliği (header ve data chunk'ının kendi CRC'si için).
pub const CRC_BITS: usize = 16;

/// Komut kodları (sistem kodu versiyon '0').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Text = 0,
    Image = 1,
    BsrResponse = 2,
    End = 3,
    BsrRequest = 4,
    Abort = 5,
    Callsign = 6,
}

impl Command {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Command::Text),
            1 => Some(Command::Image),
            2 => Some(Command::BsrResponse),
            3 => Some(Command::End),
            4 => Some(Command::BsrRequest),
            5 => Some(Command::Abort),
            6 => Some(Command::Callsign),
            _ => None,
        }
    }

    /// Bitiş/iptal/BSR sinyalleri veri chunk'ı içermez (spec, readme.txt).
    pub fn has_data_chunk(&self) -> bool {
        !matches!(self, Command::End | Command::BsrRequest | Command::Abort)
    }
}

/// Modülasyon modu (header'ın `m` alanı).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Msk = 0,
    Fsk4 = 1,
}

/// Kanal kodlama modu (header'ın `c` alanı) — veri chunk'ı için.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingMode {
    Norm = 0,
    Conv = 1,
}

pub fn sync_word_bits() -> Vec<u8> {
    SYNC_WORD.bytes().map(|b| b - b'0').collect()
}

pub fn whitening_bits() -> Vec<u8> {
    WHITENING.bytes().map(|b| b - b'0').collect()
}
