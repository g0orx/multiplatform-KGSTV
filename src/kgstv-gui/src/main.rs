//! KG-STV masaüstü arayüzü. İki mod:
//! - **RX**: bir WAV kaydını (gerçek off-air ya da kontrollü test kaydı) açar ya da varsayılan
//!   ses giriş cihazından (mikrofon/soundcard) canlı dinler, `kgstv-modem`'in MSK
//!   demodülatörü + `kgstv-core`'un çerçeve/JPEG çözücüsüyle arka planda decode eder ve
//!   görüntüyü, KG-STV'nin orijinal davranışına benzer şekilde tile tile canlı olarak çizer.
//! - **TX**: bir resim dosyası + isteğe bağlı çağrı işareti/metin alır, 320×240'a ölçekleyip
//!   300 adet 16×16 JPEG tile'ına encode eder, `kgstv-core::frame` + `kgstv-modem::session`
//!   ile tam bir KG-STV MSK oturumuna (ses) çevirir; WAV olarak kaydedilebilir ya da
//!   doğrudan hoparlörden çalınabilir.
//!
//! Ayrıca sinyal kalitesini/zamanlamasını görsel olarak değerlendirmek için bir göz diyagramı
//! (eye diagram) gösterir. Tek kod tabanı — Windows/Linux/macOS'ta native pencere olarak
//! derlenir (egui/eframe).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use image::imageops::FilterType;
use kgstv_core::constants::Command;
use kgstv_core::InfoChunk;
use kgstv_modem::{FrameResult, LiveCapture};

const TILE: usize = 16;
const IMG_W: usize = 320;
const IMG_H: usize = 240;
const TILES_X: usize = IMG_W / TILE;
const TILES_Y: usize = IMG_H / TILE;
const MAX_LOG_LINES: usize = 500;
/// TX ses üretimi için sabit örnekleme hızı (WAV kaydı ve çalma için).
const TX_SAMPLE_RATE: f64 = 44100.0;

/// Canlı modda tamponda tutulan azami ses süresi — bundan eskisi otomatik atılır.
const LIVE_BUFFER_SECONDS: f64 = 60.0;
/// Canlı modda kaç saniyede bir yeniden decode denemesi yapılır.
const LIVE_REDECODE_INTERVAL: Duration = Duration::from_millis(2000);
/// Göz diyagramının yeniden hesaplanma aralığı (her UI karesinde değil).
const EYE_UPDATE_INTERVAL: Duration = Duration::from_millis(300);
/// Göz diyagramı için kullanılan ses penceresi (son N saniye).
const EYE_WINDOW_SECONDS: f64 = 1.5;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    Rx,
    Tx,
}

enum DecodeMsg {
    /// Ses yüklendiği anda gönderilir (decode başlamadan önce) — göz diyagramının dosya
    /// modunda da çalışabilmesi için ham örnekleri UI thread'ine taşır.
    AudioReady { sample_rate: f64, samples: Arc<Vec<f64>> },
    Started { label: String, sample_rate: f64, n_samples: usize },
    Frame(FrameResult),
    Done { anchor: f64 },
    Error(String),
}

enum TxMsg {
    Progress { done: usize, total: usize },
    Done { sample_rate: f64, samples: Arc<Vec<f64>> },
    PlaybackDone,
    PlaybackError(String),
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

    // Göz diyagramı: en son yüklenen/yakalanan ses (dosyadan ya da canlı tampondan, ya da
    // TX'te üretilen ses), decode edilip edilmediğine bakılmaksızın periyodik yeniden çizilir.
    // `last_anchor`: son decode'un bulduğu senkron konumu — göz diyagramının fazını gerçek
    // sembol ızgarasına hizalamak için ŞART (bkz. `recompute_eye_diagram` dokümantasyonu;
    // anchor'sız fold rastgele/anlamsız bir faza denk gelip düz/bilgisiz bir çizim üretiyordu).
    last_audio: Option<(f64, Arc<Vec<f64>>)>,
    last_anchor: Option<f64>,
    eye_points: Vec<(f32, f32)>,
    last_eye_update: Instant,

    // TX (gönderme).
    mode: Mode,
    tx_image_rgba: Vec<u8>,
    tx_texture: Option<egui::TextureHandle>,
    tx_texture_dirty: bool,
    tx_source_name: Option<String>,
    tx_callsign: String,
    tx_message: String,
    tx_quality: u8,
    tx_conv: bool,
    tx_fsk4: bool,
    tx_encoding: bool,
    tx_progress: (usize, usize),
    tx_status: String,
    tx_audio: Option<(f64, Arc<Vec<f64>>)>,
    tx_playing: bool,
    tx_rx: Option<Receiver<TxMsg>>,
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
            last_anchor: None,
            eye_points: Vec::new(),
            last_eye_update: Instant::now(),

            mode: Mode::Rx,
            tx_image_rgba: {
                let mut buf = vec![0u8; IMG_W * IMG_H * 4];
                for px in buf.chunks_exact_mut(4) {
                    px[0] = 60;
                    px[1] = 60;
                    px[2] = 60;
                    px[3] = 255;
                }
                buf
            },
            tx_texture: None,
            tx_texture_dirty: true,
            tx_source_name: None,
            tx_callsign: String::new(),
            tx_message: String::new(),
            tx_quality: 10,
            tx_conv: false,
            tx_fsk4: false,
            tx_encoding: false,
            tx_progress: (0, 0),
            tx_status: "Bir resim seçin.".to_string(),
            tx_audio: None,
            tx_playing: false,
            tx_rx: None,
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
        self.last_anchor = None; // yeni dosya için eski anchor artık geçersiz

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
        self.last_anchor = None; // yeni anchor decode bitince gelecek — o ana kadar göz diyagramı boş kalır

        self.reset_decode_state();
        self.decoding = true;
        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);
        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            run_decode(&ctx1, &tx, "(canlı)".to_string(), sample_rate, samples);
        });
    }

    /// Göz diyagramını yeniden hesaplar. **Kritik:** faz katlaması gerçek senkron `anchor`'ına
    /// göre hizalanmalı — anchor bilinmeden `(t/tb)%1` gibi keyfi bir başlangıçtan katlamak,
    /// göz diyagramının "orta noktada açık" yapısını rastgele bir faza kaydırıp düz/anlamsız
    /// bir çizim üretir (gerçek kayıtla doğrulanan bir hataydı — bkz. `docs/protocol_spec.md`).
    /// Bu yüzden `last_anchor` bilinmeden (henüz decode tamamlanmadan) çizim yapılmaz.
    ///
    /// Ayrıca: `ToneClock::soft_metric_at`'in ham (p1-p0)/(p1+p0) değeri, MSK'nin 600Hz ton
    /// aralığının Hanning-pencereli DFT büyüklüğü (faz-uyumsuz/incoherent) bir korelatörle asla
    /// ±1'e yaklaşmaması nedeniyle DOĞAL olarak mütevazı kalır (tipik ~0.05-0.3, tek bir tb'lik
    /// pencerede iki komşu frekans arasında kalan sızıntı yüzünden) — bu bir hata değil, bu
    /// basit/incoherent yöntemin matematiksel bir özelliği (sert bit kararı yine de güvenilir,
    /// çünkü sadece işaret önemli). Göz açıklığını görsel olarak anlamlı kılmak için sabit bir
    /// `EYE_DISPLAY_GAIN` ile ölçeklenip [-1,1]'e kırpılır.
    fn recompute_eye_diagram(&mut self) {
        if self.last_eye_update.elapsed() < EYE_UPDATE_INTERVAL {
            return;
        }
        self.last_eye_update = Instant::now();
        let (Some((sample_rate, samples)), Some(anchor)) = (&self.last_audio, self.last_anchor) else {
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
        const EYE_DISPLAY_GAIN: f64 = 5.0;
        let step = tb / POINTS_PER_SYMBOL as f64;
        let mut points = Vec::new();
        let mut t = tb;
        let limit = filtered.len() as f64 - tb;
        while t < limit {
            let m = (clk.soft_metric_at(t, tb / 2.0) * EYE_DISPLAY_GAIN).clamp(-1.0, 1.0);
            // Faz, GERÇEK senkron anchor'ına göre: (mutlak_örnek_konumu - anchor) / tb.
            let abs_t = start as f64 + t;
            let phase = ((abs_t - anchor) / tb).rem_euclid(1.0) as f32;
            points.push((phase, m as f32));
            t += step;
        }
        self.eye_points = points;
    }

    fn load_tx_image(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        match image::open(&path) {
            Ok(img) => {
                let resized = img.resize_exact(IMG_W as u32, IMG_H as u32, FilterType::Lanczos3);
                self.tx_image_rgba = resized.to_rgba8().into_raw();
                self.tx_texture_dirty = true;
                self.tx_source_name = Some(name);
                self.tx_audio = None; // eski görüntüyle üretilmiş ses artık geçersiz
                self.tx_status = "Resim yüklendi — göndermeye hazır.".to_string();
            }
            Err(e) => {
                self.tx_status = format!("Resim açılamadı: {e}");
            }
        }
    }

    /// 300 tile'ı (+ isteğe bağlı çağrı işareti/metin frame'i) kodlayıp MSK sesine çevirir.
    fn start_tx_encode(&mut self, ctx: &egui::Context) {
        self.tx_encoding = true;
        self.tx_progress = (0, 0);
        self.tx_status = "Kodlanıyor...".to_string();
        let rgba = self.tx_image_rgba.clone();
        let callsign = self.tx_callsign.clone();
        let message = self.tx_message.clone();
        let quality = self.tx_quality.clamp(1, 15);
        let conv = self.tx_conv;
        let fsk4 = self.tx_fsk4;
        let modulation = fsk4 as u8;

        let (tx, rx) = mpsc::channel();
        self.tx_rx = Some(rx);
        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            let mut frames: Vec<(InfoChunk, Vec<u8>)> = Vec::new();
            if !callsign.trim().is_empty() {
                let bits = ascii_to_bits(callsign.trim());
                let hdr = InfoChunk {
                    sys: 0,
                    com: Command::Callsign as u8,
                    c: 0,
                    m: 0,
                    x: 0,
                    y: 0,
                    sc: quality,
                    size: bits.len() as u16,
                    crc: 0,
                };
                frames.push((hdr, bits));
            }
            if !message.trim().is_empty() {
                let bits = ascii_to_bits(message.trim());
                let hdr = InfoChunk {
                    sys: 0,
                    com: Command::Text as u8,
                    c: conv as u8,
                    m: modulation,
                    x: 0,
                    y: 0,
                    sc: quality,
                    size: bits.len() as u16,
                    crc: 0,
                };
                frames.push((hdr, bits));
            }
            for ty in 0..TILES_Y {
                for tx_ in 0..TILES_X {
                    let rgb = extract_tile_rgb(&rgba, tx_, ty);
                    let tile = kgstv_core::rgb_to_tile(&rgb);
                    let bits = kgstv_core::encode_tile(&tile, quality);
                    let hdr = InfoChunk {
                        sys: 0,
                        com: Command::Image as u8,
                        c: conv as u8,
                        m: modulation,
                        x: tx_ as u8,
                        y: ty as u8,
                        sc: quality,
                        size: bits.len() as u16,
                        crc: 0,
                    };
                    frames.push((hdr, bits));
                }
            }

            let tx2 = tx.clone();
            let samples = kgstv_modem::encode_session_audio_cb(&frames, TX_SAMPLE_RATE, move |done, total| {
                let _ = tx2.send(TxMsg::Progress { done, total });
            });
            let _ = tx.send(TxMsg::Done { sample_rate: TX_SAMPLE_RATE, samples: Arc::new(samples) });
            ctx1.request_repaint();
        });
    }

    fn save_tx_wav(&mut self) {
        let Some((sr, samples)) = &self.tx_audio else { return };
        let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).set_file_name("kgstv_tx.wav").save_file()
        else {
            return;
        };
        match kgstv_modem::save_mono(&path, sr.round() as u32, samples) {
            Ok(()) => self.tx_status = format!("Kaydedildi: {}", path.display()),
            Err(e) => self.tx_status = format!("WAV kaydedilemedi: {e}"),
        }
    }

    fn play_tx(&mut self, ctx: &egui::Context) {
        let Some((sr, samples)) = self.tx_audio.clone() else { return };
        if self.tx_playing {
            return;
        }
        self.tx_playing = true;
        self.tx_status = "Çalınıyor...".to_string();
        let (tx, rx) = mpsc::channel();
        self.tx_rx = Some(rx);
        let ctx1 = ctx.clone();
        std::thread::spawn(move || {
            let msg = match kgstv_modem::play_blocking(&samples, sr) {
                Ok(()) => TxMsg::PlaybackDone,
                Err(e) => TxMsg::PlaybackError(e),
            };
            let _ = tx.send(msg);
            ctx1.request_repaint();
        });
    }

    fn drain_tx_messages(&mut self) {
        let Some(rx) = &self.tx_rx else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                TxMsg::Progress { done, total } => {
                    self.tx_progress = (done, total);
                    self.tx_status = format!("Kodlanıyor... {done}/{total} frame");
                }
                TxMsg::Done { sample_rate, samples } => {
                    self.tx_encoding = false;
                    self.tx_status = format!(
                        "Hazır — {:.1} sn ses ({} örnek @ {sample_rate:.0} Hz)",
                        samples.len() as f64 / sample_rate,
                        samples.len()
                    );
                    self.last_audio = Some((sample_rate, samples.clone()));
                    // Kendi ürettiğimiz TX sesi her zaman preamble'la örnek 0'da başlıyor
                    // (bkz. encode_session_audio_cb) — anchor=0.0 tam olarak doğru.
                    self.last_anchor = Some(0.0);
                    self.tx_audio = Some((sample_rate, samples));
                }
                TxMsg::PlaybackDone => {
                    self.tx_playing = false;
                    self.tx_status = "Çalma tamamlandı.".to_string();
                }
                TxMsg::PlaybackError(e) => {
                    self.tx_playing = false;
                    self.tx_status = format!("Çalma hatası: {e}");
                }
            }
        }
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
                    let modulation = if hdr.m == 1 { "4LFSK" } else { "MSK" };
                    self.log_lines.push(format!(
                        "#{:04} {:?} x={:>2} y={:>2} sc={:>2} size={:>4} {modulation:<5} crc_ok={} viterbi_err={}",
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
                    self.last_anchor = Some(anchor);
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

/// ASCII metni MSB-first bit dizisine çevirir (çağrı işareti/metin frame'leri için).
fn ascii_to_bits(s: &str) -> Vec<u8> {
    let mut bits = Vec::with_capacity(s.len() * 8);
    for &b in s.as_bytes() {
        for i in (0..8).rev() {
            bits.push((b >> i) & 1);
        }
    }
    bits
}

/// `rgba` (320×240 RGBA8, satır-major) içinden `(tx,ty)` tile indeksindeki 16×16 RGB bloğunu
/// çıkarır (TX tarafı — `kgstv_core::rgb_to_tile`'a beslenmek üzere).
fn extract_tile_rgb(rgba: &[u8], tx: usize, ty: usize) -> [[[u8; 3]; TILE]; TILE] {
    let mut out = [[[0u8; 3]; TILE]; TILE];
    for dy in 0..TILE {
        for dx in 0..TILE {
            let px = tx * TILE + dx;
            let py = ty * TILE + dy;
            let idx = (py * IMG_W + px) * 4;
            out[dy][dx] = [rgba[idx], rgba[idx + 1], rgba[idx + 2]];
        }
    }
    out
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
        self.drain_tx_messages();

        let ctx = ui.ctx().clone();
        self.maybe_kick_live_decode(&ctx);
        self.recompute_eye_diagram();
        if self.live.is_some() || self.tx_encoding || self.tx_playing {
            // Canlı/kodlama/çalma sırasında düzenli aralıklarla yeniden çizim tetikle.
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
        if self.tx_texture_dirty {
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied([IMG_W, IMG_H], &self.tx_image_rgba);
            match &mut self.tx_texture {
                Some(tex) => tex.set(color_image, egui::TextureOptions::NEAREST),
                None => {
                    self.tx_texture = Some(ctx.load_texture(
                        "tx-image",
                        color_image,
                        egui::TextureOptions::NEAREST,
                    ));
                }
            }
            self.tx_texture_dirty = false;
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.selectable_label(self.mode == Mode::Rx, "📡 RX (Al)").clicked() {
                    self.mode = Mode::Rx;
                }
                if ui.selectable_label(self.mode == Mode::Tx, "📤 TX (Gönder)").clicked() {
                    self.mode = Mode::Tx;
                }
                ui.separator();

                match self.mode {
                    Mode::Rx => {
                        if ui
                            .add_enabled(!self.decoding && self.live.is_none(), egui::Button::new("📂 WAV Aç..."))
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                                self.open_file(&ctx, path);
                            }
                        }
                        let live_label =
                            if self.live.is_some() { "⏹ Canlı Decode Durdur" } else { "🎙 Canlı Decode Başlat" };
                        if ui.button(live_label).clicked() {
                            self.toggle_live(&ctx);
                        }
                        if self.decoding {
                            ui.spinner();
                        }
                        ui.separator();
                        ui.label(&self.status);
                    }
                    Mode::Tx => {
                        if ui.add_enabled(!self.tx_encoding, egui::Button::new("🖼 Resim Seç...")).clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Görüntü", &["png", "jpg", "jpeg", "gif", "bmp"])
                                .pick_file()
                            {
                                self.load_tx_image(path);
                            }
                        }
                        if ui
                            .add_enabled(!self.tx_encoding, egui::Button::new("⚙ Kodla"))
                            .clicked()
                        {
                            self.start_tx_encode(&ctx);
                        }
                        if ui
                            .add_enabled(self.tx_audio.is_some() && !self.tx_encoding, egui::Button::new("💾 WAV Kaydet"))
                            .clicked()
                        {
                            self.save_tx_wav();
                        }
                        if ui
                            .add_enabled(
                                self.tx_audio.is_some() && !self.tx_encoding && !self.tx_playing,
                                egui::Button::new("🔊 Çal"),
                            )
                            .clicked()
                        {
                            self.play_tx(&ctx);
                        }
                        if self.tx_encoding || self.tx_playing {
                            ui.spinner();
                        }
                        ui.separator();
                        ui.label(&self.tx_status);
                    }
                }
            });
        });

        match self.mode {
            Mode::Rx => {
                egui::Panel::left("log").resizable(true).default_size(420.0).show(ui, |ui| {
                    ui.heading("Frame Günlüğü");
                    ui.label(format!("Kaynak: {}", self.current_file.as_deref().unwrap_or("(yok)")));
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
            }
            Mode::Tx => {
                egui::Panel::left("txpanel").resizable(true).default_size(420.0).show(ui, |ui| {
                    ui.heading("Gönderim Ayarları");
                    ui.label(format!("Kaynak: {}", self.tx_source_name.as_deref().unwrap_or("(seçilmedi)")));
                    ui.separator();
                    ui.label("Çağrı işareti (opsiyonel):");
                    ui.text_edit_singleline(&mut self.tx_callsign);
                    ui.label("Mesaj (opsiyonel):");
                    ui.text_edit_multiline(&mut self.tx_message);
                    ui.add_space(8.0);
                    ui.add(egui::Slider::new(&mut self.tx_quality, 1..=15).text("JPEG kalite (sc)"));
                    ui.checkbox(&mut self.tx_conv, "Konvolüsyonel kodlama (CONV)");
                    ui.checkbox(&mut self.tx_fsk4, "4L-FSK modülasyonu (metin+görüntü için — çağrı işareti hep MSK)");
                    ui.add_space(8.0);
                    let (done, total) = self.tx_progress;
                    if total > 0 {
                        ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(format!("{done}/{total}")));
                    }
                    ui.separator();
                    ui.label("300 tile (20×15), 1200 baud MSK — gerçek KG-STV ile aynı çerçeveleme.");
                });
            }
        }

        egui::Panel::bottom("eye").resizable(true).default_size(180.0).show(ui, |ui| {
            ui.heading("Göz Diyagramı (Eye Diagram) — MSK yumuşak karar metriği");
            let (rect, _resp) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 130.0), egui::Sense::hover());
            paint_eye_diagram(ui, rect, &self.eye_points);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.mode {
            Mode::Rx => {
                ui.heading("Görüntü (320×240)");
                if let Some(tex) = &self.texture {
                    let scale = 2.0;
                    ui.add(
                        egui::Image::new(tex)
                            .fit_to_exact_size(egui::vec2(IMG_W as f32 * scale, IMG_H as f32 * scale)),
                    );
                }
            }
            Mode::Tx => {
                ui.heading("Gönderilecek Görüntü (320×240)");
                if let Some(tex) = &self.tx_texture {
                    let scale = 2.0;
                    ui.add(
                        egui::Image::new(tex)
                            .fit_to_exact_size(egui::vec2(IMG_W as f32 * scale, IMG_H as f32 * scale)),
                    );
                }
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
