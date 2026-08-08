//! Geçici tanı aracı: bir WAV'ı decode edip HER görüntü tile'ının x,y,sc ve 6 ham (dequant
//! ÖNCESİ) DC katsayısını CSV olarak yazdırır — doğru kuantizasyon ölçeğini gerçek referans
//! görüntüyle karşılaştırarak (Python) ampirik olarak türetmek için.
use kgstv_core::constants::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let in_path = &args[1];
    let audio = kgstv_modem::load_mono(in_path).expect("wav okunamadı");
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
