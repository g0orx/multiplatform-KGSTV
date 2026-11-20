//! kgstv-core: platform-independent core of the KG-STV protocol.
//!
//! Core library that reproduces the protocol of JJ0OBZ's original KG-STV software (Windows-only),
//! as verified through a translation of its original readme.txt and bit-level reverse engineering
//! of real off-air audio recordings. See `docs/protocol_spec.md`.
//!
//! Scope (in this version): framing (preamble/sync), rate-1/2 K=7 convolutional code, 127-bit
//! whitening, CRC-16/AUG-CCITT, 54-bit info chunk (header), and a 16×16 JPEG-compatible image
//! block codec (encode+decode) — all verified against real recordings for MSK mode. 4L-FSK
//! demodulation is still open (see protocol_spec.md, "Still Uncertain" sections).

pub mod constants;
pub mod crc;
pub mod conv;
pub mod whiten;
pub mod header;
pub mod frame;
pub mod jpeg_tables;
pub mod jpeg;

pub use constants::{Command, CodingMode, Modulation};
pub use header::InfoChunk;
pub use jpeg::{decode_tile, decode_tile_raw_dc, encode_tile, rgb_to_tile, tile_to_rgb, Tile};
