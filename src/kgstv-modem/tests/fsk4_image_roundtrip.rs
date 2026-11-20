//! End-to-end regression test for a full 4L-FSK image session at the GUI's TX sample rate
//! (44.1 kHz): detailed tiles produce large JPEG payloads (~300-750 bits, up to ~800 4L-FSK
//! symbols with CONV), which used to fail CRC on a clean WAV because the modulator's phase
//! drifted off the CPM trellis (see `fsk4::modulate_fsk4`).

use kgstv_core::{encode_tile, rgb_to_tile, Command, InfoChunk};
use kgstv_modem::{decode_wav_sequential, encode_session_audio};

const TILE: usize = 16;

fn busy_tile(tx: usize, ty: usize) -> [[[u8; 3]; TILE]; TILE] {
    // Hard-edged, high-contrast detail (like black text on red) so every tile is expensive.
    let mut rgb = [[[0u8; 3]; TILE]; TILE];
    for (y, row) in rgb.iter_mut().enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let on = ((x * 3 + y * 5 + tx * 7 + ty * 11) / 3) % 2 == 0;
            *px = if on { [0, 0, 0] } else { [255, (x * 16) as u8, (y * 16) as u8] };
        }
    }
    rgb
}

fn roundtrip(conv: bool) {
    let sr = 44100.0;
    let quality = 10u8;
    let (tiles_x, tiles_y) = (20usize, 3usize);
    let mut frames = Vec::new();
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let bits = encode_tile(&rgb_to_tile(&busy_tile(tx, ty)), quality);
            let hdr = InfoChunk {
                sys: 0,
                com: Command::Image as u8,
                c: conv as u8,
                m: 1,
                x: tx as u8,
                y: ty as u8,
                sc: quality,
                size: bits.len() as u16,
                crc: 0,
            };
            frames.push((hdr, bits));
        }
    }
    let max_size = frames.iter().map(|(h, _)| h.size).max().unwrap();
    assert!(max_size > 600, "test tiles should be large (max {max_size} bits)");

    let audio = encode_session_audio(&frames, sr);
    let (_anchor, decoded) = decode_wav_sequential(&audio, sr, 2000, frames.len() + 10);
    let ok: Vec<_> = decoded
        .iter()
        .filter(|f| f.header.com == Command::Image as u8 && f.data_crc_ok)
        .map(|f| (f.header.x, f.header.y))
        .collect();
    assert_eq!(ok.len(), frames.len(), "conv={conv}: only {}/{} tiles decoded", ok.len(), frames.len());
}

#[test]
fn fsk4_conv_large_tiles_44k1() {
    roundtrip(true);
}

#[test]
fn fsk4_norm_large_tiles_44k1() {
    roundtrip(false);
}
