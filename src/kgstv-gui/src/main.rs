//! KG-STV masaüstü arayüzü: bir WAV kaydını (gerçek off-air ya da kontrollü test kaydı) açar
//! ya da varsayılan ses giriş cihazından (mikrofon/soundcard) canlı dinler,
//! `kgstv-modem`'in MSK demodülatörü + `kgstv-core`'un çerçeve/JPEG çözücüsüyle arka planda
//! decode eder ve görüntüyü, KG-STV'nin orijinal davranışına benzer şekilde tile tile canlı
//! olarak çizer. Ayrıca sinyal kalitesini/zamanlamasını görsel olarak değerlendirmek için bir
//! göz diyagramı (eye diagram) gösterir. Tek kod tabanı — Windows/Linux/macOS'ta native pencere
//! olarak derlenir (egui/eframe).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use kgstv_core::constants::Command;
use kgstv_modem::{FrameResult, LiveCapture};

const TILE: usize = 16;
const IMG_W: usize = 320;
const IMG_H: usize = 240;
const MAX_LOG_LINES: usize = 500;

/// Canlı modda tamponda tutulan azami ses süresi — bundan eskisi otomatik atılır.
const LIVE_BUFFER_SECONDS: f64 = 60.0;
/// Canlı modda kaç saniyede bir yeniden decode denemesi yapılır.
const LIVE_REDECODE_INTERVAL: Duration = Duration::from_millis(2000);
/// Göz diyagramının yeniden hesaplanma aralığı (her UI karesinde değil).
const EYE_UPDATE_INTERVAL: Duration = Duration::from_millis(300);
/// Göz diyagramı için kullanılan ses penceresi (son N saniye).
const EYE_WINDOW_SECONDS: f64 = 1.5;

enum DecodeMsg {
    /// Ses yüklendiği anda gönderilir (decode başlamadan önce) — göz diyagramının dosya
    /// modunda da çalışabilmesi için ham örnekleri UI thread'ine taşır.
    AudioReady { sample_rate: f64, samples: Arc<Vec<f64>> },
    Started { label: String, sample_rate: f64, n_samples: usize },
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

    // Canlı decode.
    live: Option<LiveCapture>,
    last_live_decode: Instant,

    // Göz diyagramı: en son yüklenen/yakalanan ses (dosyadan ya da canlı tampondan),
    // decode edilip edilmediğine bakılmaksızın periyodik olarak yeniden çizilir.
    last_audio: Option<(f64, Arc<Vec<f64>>)>,
    eye_points: Vec<(f32, f32)>,
    last_eye_update: Instant,
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
            status: "Hazır — bir WAV kaydı açın ya da canlı decode'u başlatın.".to_string(),
            rx: None,
            decoding: false,
            tiles_ok: 0,
            tiles_total: 0,
            frames_total: 0,
            current_file: None,
            live: None,
            last_live_decode: Instant::now(),
            last_audio: None,
            eye_points: Vec::new(),
            last_eye_update: Instant::now(),
        }
    }
}

impl KgstvApp {
    fn reset_decode_state(&mut self) {
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
    }

    fn open_file(&mut self, ctx: &egui::Context, path: PathBuf) {
        let path_str = path.display().to_string();
        self.current_file = Some(path_str.clone());
        self.status = format!("Yükleniyor: {path_str}");
        self.reset_decode_state();
        self.decoding = true;
        self.live = None; // dosya modu canlı yakalamayı durdurur

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
            let samples = Arc::new(audio.samples);
            let _ = tx.send(DecodeMsg::AudioReady { sample_rate: audio.sample_rate, samples: samples.clone() });
            run_decode(&ctx1, &tx, path_str, audio.sample_rate, samples);
        });
    }

    fn toggle_live(&mut self, ctx: &egui::Context) {
        if self.live.is_some() {
            self.live = None;
            self.status = "Canlı decode durduruldu.".to_string();
            return;
        }
        match LiveCapture::start(LIVE_BUFFER_SECONDS) {
            Ok(cap) => {
                self.status = format!("Canlı dinleniyor @ {:.0} Hz — bekleniyor...", cap.sample_rate);
                self.current_file = Some("(canlı giriş)".to_string());
                self.reset_decode_state();
                self.live = Some(cap);
                self.last_live_decode = Instant::now() - LIVE_REDECODE_INTERVAL; // hemen tetikle
                let _ = ctx;
            }
            Err(e) => {
                self.status = format!("Canlı yakalama başlatılamadı: {e}");
            }
        }
    }

    /// Canlı modda periyodik olarak biriken tamponu yeniden decode etmeyi dener
    /// (önceki decode bitmişse). Bir sonraki pas, tamponun o anki içeriğine göre baştan
    /// senkron arar — bu yüzden görüntü/günlük her pas başında sıfırlanır.
    fn maybe_kick_live_decode(&mut self, ctx: &egui::Context) {
        let Some(live) = &self.live else { return };
        if self.decoding || self.last_live_decode.elapsed() < LIVE_REDECODE_INTERVAL {
            return;
        }
        if live.len() < 4096 {
            return; // henüz anlamlı bir decode denemesi için yeterli örnek yok
        }
        self.last_live_decode = Instant::now();
        let sample_rate = live.sample_rate;
        let samples = Arc::new(live.snapshot());
        self.last_audio = Some((sample_rate, samples.clone()));

        self.reset_decode_state();
        self.decoding = true;
        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);
        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            run_decode(&ctx1, &tx, "(canlı)".to_string(), sample_rate, samples);
        });
    }

    fn recompute_eye_diagram(&mut self) {
        if self.last_eye_update.elapsed() < EYE_UPDATE_INTERVAL {
            return;
        }
        self.last_eye_update = Instant::now();
        let Some((sample_rate, samples)) = &self.last_audio else {
            self.eye_points.clear();
            return;
        };
        let sr = *sample_rate;
        let n = samples.len();
        let take = ((EYE_WINDOW_SECONDS * sr) as usize).min(n);
        if take < 256 {
            self.eye_points.clear();
            return;
        }
        let start = n - take;
        let filtered = kgstv_modem::bandpass_filtfilt(&samples[start..], sr, 900.0, 2200.0);
        let clk = kgstv_modem::ToneClock::new(&filtered, sr);
        let tb = clk.tb();
        if tb < 1.0 {
            self.eye_points.clear();
            return;
        }
        const POINTS_PER_SYMBOL: usize = 24;
        let step = tb / POINTS_PER_SYMBOL as f64;
        let mut points = Vec::new();
        let mut t = tb;
        let limit = filtered.len() as f64 - tb;
        while t < limit {
            let m = clk.soft_metric_at(t);
            let phase = ((t / tb) % 1.0) as f32;
            points.push((phase, m as f32));
            t += step;
        }
        self.eye_points = points;
    }

    fn drain_messages(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                DecodeMsg::AudioReady { sample_rate, samples } => {
                    self.last_audio = Some((sample_rate, samples));
                }
                DecodeMsg::Started { label, sample_rate, n_samples } => {
                    self.status = format!(
                        "{label} decode ediliyor: {n_samples} örnek @ {sample_rate:.0} Hz ({:.1} sn)",
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

/// Ortak decode çalıştırıcısı: hem dosya modu hem canlı mod tarafından, zaten arka planda
/// çalışan bir thread içinden çağrılır. Senkron bulur, frame frame yürür, her frame'i
/// `tx` üzerinden UI thread'ine bildirir.
fn run_decode(ctx: &egui::Context, tx: &Sender<DecodeMsg>, label: String, sample_rate: f64, samples: Arc<Vec<f64>>) {
    let _ = tx.send(DecodeMsg::Started { label, sample_rate, n_samples: samples.len() });
    ctx.request_repaint();

    // Senkronun kayıt cihazı başlatma gecikmesi/sessizlik yüzünden ilk saniyelerde
    // olmayabileceğini göz önünde bulundurarak ilk 15 saniyeyi tara.
    let coarse_search_len = ((15.0 * sample_rate) as i64).min(samples.len() as i64);

    let tx2 = tx.clone();
    let ctx2 = ctx.clone();
    let anchor = kgstv_modem::decode_wav_sequential_cb(&samples, sample_rate, coarse_search_len, 5000, move |fr| {
        let _ = tx2.send(DecodeMsg::Frame(fr));
        ctx2.request_repaint();
    });
    let _ = tx.send(DecodeMsg::Done { anchor });
    ctx.request_repaint();
}

/// Göz diyagramını (`points`: (faz∈[0,1], yumuşak metrik∈[-1,1])) verilen dikdörtgen içine
/// çizer. Ardışık noktalar aynı "iz" (trace) içinde birleştirilir; faz sarmalandığında
/// (1.0'dan 0.0'a döndüğünde) yeni bir iz başlatılır — klasik üst üste binen çizgiler
/// görünümü için.
fn paint_eye_diagram(ui: &mut egui::Ui, rect: egui::Rect, points: &[(f32, f32)]) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(18));

    let to_screen = |phase: f32, m: f32| -> egui::Pos2 {
        let x = rect.left() + phase * rect.width();
        // m: -1..1 -> y: alt..üst (m=+1 yukarıda)
        let y = rect.center().y - (m * 0.45) * rect.height();
        egui::pos2(x, y)
    };

    // Izgara: -1, 0, +1 seviyeleri.
    for &m in &[-1.0f32, 0.0, 1.0] {
        let y = to_screen(0.0, m).y;
        painter.hline(rect.x_range(), y, egui::Stroke::new(1.0, egui::Color32::from_gray(50)));
    }
    painter.text(
        rect.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        "bit=1",
        egui::FontId::monospace(10.0),
        egui::Color32::from_gray(120),
    );
    painter.text(
        rect.left_bottom() + egui::vec2(4.0, -12.0),
        egui::Align2::LEFT_TOP,
        "bit=0",
        egui::FontId::monospace(10.0),
        egui::Color32::from_gray(120),
    );

    if points.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "(ses yok)",
            egui::FontId::proportional(14.0),
            egui::Color32::from_gray(90),
        );
        return;
    }

    let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(80, 220, 120, 110));
    let mut trace: Vec<egui::Pos2> = Vec::new();
    let mut last_phase = -1.0f32;
    for &(phase, m) in points {
        if phase < last_phase - 0.5 {
            if trace.len() > 1 {
                painter.add(egui::Shape::line(trace.clone(), stroke));
            }
            trace.clear();
        }
        trace.push(to_screen(phase, m));
        last_phase = phase;
    }
    if trace.len() > 1 {
        painter.add(egui::Shape::line(trace, stroke));
    }
}

impl eframe::App for KgstvApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_messages();

        let ctx = ui.ctx().clone();
        self.maybe_kick_live_decode(&ctx);
        self.recompute_eye_diagram();
        if self.live.is_some() {
            // Canlı modda düzenli aralıklarla yeniden çizim tetikle (tampon dolarken günceller).
            ctx.request_repaint_after(Duration::from_millis(150));
        }

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
                    .add_enabled(!self.decoding && self.live.is_none(), egui::Button::new("📂 WAV Aç..."))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                        self.open_file(&ctx, path);
                    }
                }
                let live_label = if self.live.is_some() { "⏹ Canlı Decode Durdur" } else { "🎙 Canlı Decode Başlat" };
                if ui.button(live_label).clicked() {
                    self.toggle_live(&ctx);
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
                "Kaynak: {}",
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

        egui::Panel::bottom("eye").resizable(true).default_size(180.0).show(ui, |ui| {
            ui.heading("Göz Diyagramı (Eye Diagram) — MSK yumuşak karar metriği");
            let (rect, _resp) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 130.0), egui::Sense::hover());
            paint_eye_diagram(ui, rect, &self.eye_points);
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
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "KGSTV — Cross-Platform",
        options,
        Box::new(|_cc| Ok(Box::new(KgstvApp::default()))),
    )
}
