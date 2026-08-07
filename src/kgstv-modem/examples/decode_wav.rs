//! Gerçek/senkron bir WAV kaydını uçtan uca decode eden basit CLI:
//! ses -> sync arama -> ardışık frame yürüyüşü -> (Image ise) JPEG tile decode -> PPM görüntü.
//!
//! Kullanım: cargo run --example decode_wav -- <girdi.wav> [çıktı.ppm]
//!
//! Not: bu, `research/decoder/decode_session.py`'deki bağımsız-aday-tarama yaklaşımı değil,
//! basit ardışık (sıralı) frame yürüyüşüdür — temiz/kısa kayıtlar (ör. OH2UDS kontrollü test
//! görüntüleri) için yeterlidir; gürültülü gerçek off-air kayıtlarda senkron kaybında durur.

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
        eprintln!("kullanım: decode_wav <girdi.wav> [çıktı.ppm]");
        return ExitCode::FAILURE;
    }
    let in_path = &args[1];
    let out_path = args.get(2).map(|s| s.as_str()).unwrap_or("decoded_out.ppm");

    let audio = match kgstv_modem::load_mono(in_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("WAV okunamadı: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "{} yüklendi: {} örnek @ {} Hz ({:.1} sn)",
        in_path,
        audio.samples.len(),
        audio.sample_rate,
        audio.samples.len() as f64 / audio.sample_rate
    );

    // Senkron başlangıcı kayıt cihazının başlatma gecikmesi/sessizlik yüzünden kaydın ilk
    // saniyelerinde OLMAYABİLİR — ilk 15 saniyeyi tara (gerçek kayıtlarda gözlenen ~3sn
    // gecikmeyi rahatça kapsar).
    let coarse_search_len = ((15.0 * audio.sample_rate) as i64).min(audio.samples.len() as i64);
    let (anchor, frames) =
        kgstv_modem::decode_wav_sequential(&audio.samples, audio.sample_rate, coarse_search_len, 2000);
    println!("senkron anchor: {anchor:.2} örnek, {} frame bulundu", frames.len());

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

    println!("görüntü tile'ları: {tiles_ok}/{tiles_total} decode edildi");
    match write_ppm(out_path, &canvas) {
        Ok(()) => println!("yazıldı: {out_path}"),
        Err(e) => {
            eprintln!("PPM yazılamadı: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
