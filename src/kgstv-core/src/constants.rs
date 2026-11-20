//! Constants of the KG-STV protocol.
//! Source: docs/protocol_spec.md (translated from JJ0OBZ's original readme.txt,
//! verified at bit level against real off-air recordings).

/// Symbol rate (baud). Same for MSK and 4L-FSK.
pub const BAUD: f64 = 1200.0;

/// MSK frequencies: [bit0 (space), bit1 (mark)]
pub const MSK_FREQS: [f64; 2] = [1200.0, 1800.0];

/// 4L-FSK frequencies: level 0..3 -> Hz. Level = 2*bit_high + bit_low.
/// '00'=1200Hz '01'=1400Hz '10'=1600Hz '11'=1800Hz
pub const FSK4_FREQS: [f64; 4] = [1200.0, 1400.0, 1600.0, 1800.0];

/// Preamble: 256 bits, repeating "01" (before sync, for clock recovery).
pub const PREAMBLE_LEN: usize = 256;

/// 63-bit sync word (M-sequence), verbatim from readme.txt.
pub const SYNC_WORD: &str = "000011100001001000110110010110101110111100110001010100111111010";

/// 127-bit whitening M-sequence code. At the start of each chunk (header, data) the
/// index restarts from zero (NOT a global/continuous counter) — verified against a real recording.
pub const WHITENING: &str = concat!(
    "1110110011000100100111001111100100000100011010101001101101001010",
    "000101100001100101111111010110111011110001110100010101110000001"
);

/// Raw length of the info chunk (header), in bits.
pub const HEADER_RAW_BITS: usize = 54;

/// The header is always encoded with the rate-1/2 K=7 convolutional code (independent of the
/// data chunk's own c/m setting) -> 108 encoded bits.
pub const HEADER_ENC_BITS: usize = HEADER_RAW_BITS * 2;

/// CRC field width (for the header's and the data chunk's own CRC).
pub const CRC_BITS: usize = 16;

/// Command codes (system code version '0').
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

    /// End/abort/BSR signals contain no data chunk (spec, readme.txt).
    pub fn has_data_chunk(&self) -> bool {
        !matches!(self, Command::End | Command::BsrRequest | Command::Abort)
    }
}

/// Modulation mode (the header's `m` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modulation {
    Msk = 0,
    Fsk4 = 1,
}

/// Channel coding mode (the header's `c` field) — for the data chunk.
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
