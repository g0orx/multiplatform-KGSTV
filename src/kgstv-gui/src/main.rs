//! KG-STV masaüstü arayüzü: bir WAV kaydını (gerçek off-air ya da kontrollü test kaydı)
//! açar, `kgstv-modem`'in MSK demodülatörü + `kgstv-core`'un çerçeve/JPEG çözücüsüyle
//! arka planda decode eder ve görüntüyü, KG-STV'nin orijinal davranışına benzer şekilde
//! tile tile canlı olarak çizer. Tek kod tabanı — Windows/Linux/macOS'ta native pencere
//! olarak derlenir (egui/eframe).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use kgstv_core::constants::Command;
use kgstv_modem::FrameResult;

const TILE: usize = 16;
const IMG_W: usize = 320;
const IMG_H: usize = 240;
const MAX_LOG_LINES: usize = 500;

enum DecodeMsg {
    Started { path: String, sample_rate: f64, n_samples: usize },
    Frame(FrameResult),
    Done { anchor: f64 },
    Error(String),
}

struct KgstvApp {
    image_rgba: Vec<u8>,
    texture: Option<egui::TextureHandle>,
    texture_dirty: bool,
    log_lines: Vec<String>,
    status: String,
    rx: Option<Receiver<DecodeMsg>>,
    decoding: bool,
    tiles_ok: usize,
    tiles_total: usize,
    frames_total: usize,
    current_file: Option<String>,
}

impl Default for KgstvApp {
    fn default() -> Self {
        // Başlangıç tuvali: KG-STV ekranındaki gibi düz koyu gri "henüz gelmedi" arka planı.
        let mut image_rgba = vec![0u8; IMG_W * IMG_H * 4];
        for px in image_rgba.chunks_exact_mut(4) {
            px[0] = 40;
            px[1] = 40;
            px[2] = 40;
            px[3] = 255;
        }
        KgstvApp {
            image_rgba,
            texture: None,
            texture_dirty: true,
            log_lines: Vec::new(),
            status: "Hazır — bir WAV kaydı açın.".to_string(),
            rx: None,
            decoding: false,
            tiles_ok: 0,
            tiles_total: 0,
            frames_total: 0,
            current_file: None,
        }
    }
}

impl KgstvApp {
    fn open_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        let path_str = path.display().to_string();
        self.current_file = Some(path_str.clone());
        self.status = format!("Yükleniyor: {path_str}");
        self.log_lines.clear();
        self.tiles_ok = 0;
        self.tiles_total = 0;
        self.frames_total = 0;
        for px in self.image_rgba.chunks_exact_mut(4) {
            px[0] = 40;
            px[1] = 40;
            px[2] = 40;
            px[3] = 255;
        }
        self.texture_dirty = true;
        self.decoding = true;

        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);

        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            let audio = match kgstv_modem::load_mono(&path) {
                Ok(a) => a,
                Err(e) => {
                    let _ = tx.send(DecodeMsg::Error(format!("WAV okunamadı: {e}")));
                    ctx1.request_repaint();
                    return;
                }
            };
            let _ = tx.send(DecodeMsg::Started {
                path: path_str,
                sample_rate: audio.sample_rate,
                n_samples: audio.samples.len(),
            });
            ctx1.request_repaint();

            // Senkronun kayıt cihazı başlatma gecikmesi/sessizlik yüzünden ilk saniyelerde
            // olmayabileceğini göz önünde bulundurarak ilk 15 saniyeyi tara.
            let coarse_search_len =
                ((15.0 * audio.sample_rate) as i64).min(audio.samples.len() as i64);

            let tx2 = tx.clone();
            let ctx2 = ctx1.clone();
            let anchor = kgstv_modem::decode_wav_sequential_cb(
                &audio.samples,
                audio.sample_rate,
                coarse_search_len,
                5000,
                move |fr| {
                    let _ = tx2.send(DecodeMsg::Frame(fr));
                    ctx2.request_repaint();
                },
            );
            let _ = tx.send(DecodeMsg::Done { anchor });
            ctx1.request_repaint();
        });
    }

    fn drain_messages(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                DecodeMsg::Started { path, sample_rate, n_samples } => {
                    self.status = format!(
                        "{path} decode ediliyor: {n_samples} örnek @ {sample_rate:.0} Hz ({:.1} sn)",
                        n_samples as f64 / sample_rate
                    );
                }
                DecodeMsg::Frame(fr) => {
                    self.frames_total += 1;
                    let hdr = &fr.header;
                    let cmd = Command::from_u8(hdr.com);
                    self.log_lines.push(format!(
                        "#{:04} {:?} x={:>2} y={:>2} sc={:>2} size={:>4} crc_ok={} viterbi_err={}",
                        self.frames_total, cmd, hdr.x, hdr.y, hdr.sc, hdr.size, fr.data_crc_ok, fr.header_metric
                    ));
                    if self.log_lines.len() > MAX_LOG_LINES {
                        self.log_lines.remove(0);
                    }
                    if cmd == Some(Command::Image) {
                        self.tiles_total += 1;
                        if let Some(data_bits) = &fr.data_bits {
                            if let Some(tile) = kgstv_core::decode_tile(data_bits, hdr.sc) {
                                let rgb = kgstv_core::tile_to_rgb(&tile);
                                let px0 = hdr.x as usize * TILE;
                                let py0 = hdr.y as usize * TILE;
                                if px0 + TILE <= IMG_W && py0 + TILE <= IMG_H {
                                    for dy in 0..TILE {
                                        for dx in 0..TILE {
                                            let [r, g, b] = rgb[dy][dx];
                                            let idx = ((py0 + dy) * IMG_W + (px0 + dx)) * 4;
                                            self.image_rgba[idx] = r;
                                            self.image_rgba[idx + 1] = g;
                                            self.image_rgba[idx + 2] = b;
                                            self.image_rgba[idx + 3] = 255;
                                        }
                                    }
                                    self.texture_dirty = true;
                                    self.tiles_ok += 1;
                                }
                            }
                        }
                    }
                }
                DecodeMsg::Done { anchor } => {
                    self.decoding = false;
                    self.status = format!(
                        "Tamamlandı — senkron anchor={anchor:.1} örnek, {} frame, tile: {}/{}",
                        self.frames_total, self.tiles_ok, self.tiles_total
                    );
                }
                DecodeMsg::Error(e) => {
                    self.decoding = false;
                    self.status = format!("Hata: {e}");
                }
            }
        }
    }
}

impl eframe::App for KgstvApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_messages();

        let ctx = ui.ctx().clone();
        if self.texture_dirty {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([IMG_W, IMG_H], &self.image_rgba);
            match &mut self.texture {
                Some(tex) => tex.set(color_image, egui::TextureOptions::NEAREST),
                None => {
                    self.texture = Some(ctx.load_texture(
                        "decoded-image",
                        color_image,
                        egui::TextureOptions::NEAREST,
                    ));
                }
            }
            self.texture_dirty = false;
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.decoding, egui::Button::new("📂 WAV Aç..."))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                        self.open_file(&ctx, path);
                    }
                }
                if self.decoding {
                    ui.spinner();
                }
                ui.separator();
                ui.label(&self.status);
            });
        });

        egui::Panel::left("log").resizable(true).default_size(420.0).show(ui, |ui| {
            ui.heading("Frame Günlüğü");
            ui.label(format!(
                "Dosya: {}",
                self.current_file.as_deref().unwrap_or("(yok)")
            ));
            ui.label(format!(
                "Frame: {}   Tile: {}/{}",
                self.frames_total, self.tiles_ok, self.tiles_total
            ));
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(
                ui,
                |ui| {
                    for line in &self.log_lines {
                        ui.monospace(line);
                    }
                },
            );
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Görüntü (320×240)");
            if let Some(tex) = &self.texture {
                let scale = 2.0;
                ui.add(
                    egui::Image::new(tex)
                        .fit_to_exact_size(egui::vec2(IMG_W as f32 * scale, IMG_H as f32 * scale)),
                );
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "KGSTV — Cross-Platform",
        options,
        Box::new(|_cc| Ok(Box::new(KgstvApp::default()))),
    )
}
