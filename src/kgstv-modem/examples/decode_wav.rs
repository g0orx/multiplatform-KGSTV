//! Simple CLI that decodes a real/synthetic WAV recording end to end:
//! audio -> sync search -> sequential frame walk -> (if Image) JPEG tile decode -> PPM image.
//!
//! Usage: cargo run --example decode_wav -- <input.wav> [output.ppm]
//!
//! Note: this is a simple sequential frame walk, not the independent-candidate scanning approach
//! of `research/decoder/decode_session.py` — sufficient for clean/short recordings (e.g. the OH2UDS
//! controlled test images); on noisy real off-air recordings it stops on loss of sync.

use std::env;
use std::process::ExitCode;

use kgstv_core::constants::Command;

const TILE: usize = 16;
const IMG_W: usize = 320;
const IMG_H: usize = 240;

fn write_ppm(path: &str, rgb: &[[[u8; 3]; IMG_W]; IMG_H]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    write!(f, "P6\n{IMG_W} {IMG_H}\n255\n")?;
    for row in rgb.iter() {
        for px in row.iter() {
            f.write_all(px)?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: decode_wav <input.wav> [output.ppm]");
        return ExitCode::FAILURE;
    }
    let in_path = &args[1];
    let out_path = args.get(2).map(|s| s.as_str()).unwrap_or("decoded_out.ppm");

    let audio = match kgstv_modem::load_mono(in_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not read WAV: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{} loaded: {} samples @ {} Hz ({:.1} s)",
        in_path,
        audio.samples.len(),
        audio.sample_rate,
        audio.samples.len() as f64 / audio.sample_rate
    );

    // The sync start may NOT be in the first seconds of the recording due to the recorder's
    // start-up delay/silence — scan the first 15 seconds (comfortably covers the ~3 s delay
    // observed in real recordings).
    let coarse_search_len = ((15.0 * audio.sample_rate) as i64).min(audio.samples.len() as i64);
    let (anchor, frames) =
        kgstv_modem::decode_wav_sequential(&audio.samples, audio.sample_rate, coarse_search_len, 2000);
    println!("sync anchor: {anchor:.2} samples, {} frames found", frames.len());

    let mut canvas = [[[0u8; 3]; IMG_W]; IMG_H];
    let mut tiles_ok = 0usize;
    let mut tiles_total = 0usize;

    for (i, fr) in frames.iter().enumerate() {
        let hdr = &fr.header;
        let cmd = Command::from_u8(hdr.com);
        println!(
            "frame {i}: com={:?} x={} y={} sc={} size={} data_crc_ok={} header_metric={}",
            cmd, hdr.x, hdr.y, hdr.sc, hdr.size, fr.data_crc_ok, fr.header_metric
        );
        if cmd == Some(Command::Image) {
            tiles_total += 1;
            if let Some(data_bits) = &fr.data_bits {
                if let Some(tile) = kgstv_core::decode_tile(data_bits, hdr.sc) {
                    let rgb = kgstv_core::tile_to_rgb(&tile);
                    let px0 = hdr.x as usize * TILE;
                    let py0 = hdr.y as usize * TILE;
                    if px0 + TILE <= IMG_W && py0 + TILE <= IMG_H {
                        for dy in 0..TILE {
                            for dx in 0..TILE {
                                canvas[py0 + dy][px0 + dx] = rgb[dy][dx];
                            }
                        }
                        tiles_ok += 1;
                    }
                }
            }
        }
    }

    println!("image tiles: {tiles_ok}/{tiles_total} decoded");
    match write_ppm(out_path, &canvas) {
        Ok(()) => println!("written: {out_path}"),
        Err(e) => {
            eprintln!("could not write PPM: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
