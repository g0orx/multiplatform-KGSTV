//! Temporary diagnostic tool: decodes a WAV and prints the x,y,sc and the 6 raw (BEFORE
//! dequantization) DC coefficients of EVERY image tile as CSV — to empirically derive the correct
//! quantization scale by comparing against a real reference image (Python).
use kgstv_core::constants::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let in_path = &args[1];
    let audio = kgstv_modem::load_mono(in_path).expect("could not read wav");
    let coarse_search_len = ((15.0 * audio.sample_rate) as i64).min(audio.samples.len() as i64);
    let (_anchor, frames) = kgstv_modem::decode_wav_sequential(&audio.samples, audio.sample_rate, coarse_search_len, 2000);

    println!("x,y,sc,size,dc_y0,dc_y1,dc_y2,dc_y3,dc_cr,dc_cb");
    for fr in &frames {
        let hdr = &fr.header;
        if Command::from_u8(hdr.com) != Some(Command::Image) || !fr.data_crc_ok {
            continue;
        }
        let bits = fr.data_bits.as_ref().unwrap();
        if let Some(raw) = kgstv_core::decode_tile_raw_dc(bits) {
            println!(
                "{},{},{},{},{},{},{},{},{},{}",
                hdr.x, hdr.y, hdr.sc, hdr.size, raw[0], raw[1], raw[2], raw[3], raw[4], raw[5]
            );
        }
    }
}
