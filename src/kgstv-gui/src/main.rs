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
use std::sync::atomic::{AtomicUsize, Ordering};
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
/// Gerçek zamanlı WAV oynatma simülasyonunda kaç saniyede bir yeniden decode denemesi yapılır
/// (canlı mikrofondan daha sık — kayıt önceden bilindiği için daha canlı bir izleme deneyimi).
const WAV_SIM_REDECODE_INTERVAL: Duration = Duration::from_millis(700);
/// Göz diyagramının yeniden hesaplanma aralığı (her UI karesinde değil).
const EYE_UPDATE_INTERVAL: Duration = Duration::from_millis(300);
/// Göz diyagramı için kullanılan ses penceresi (son N saniye). Faz katlaması, TEK bir
/// (dosyanın başına yakın bulunan) `anchor`'a ve NOMİNAL sembol süresine (`sr/1200`, kayıt
/// boyunca sabit varsayılır) dayanıyor — gerçek off-air kayıtlarda verici ile kayıt cihazının
/// örnekleme saatleri arasındaki (küçük ama gerçek) fark, pencere ne kadar uzunsa o kadar
/// biriken bir faz kaymasına yol açıyor (temiz/sentetik OH2UDS test sinyallerinde bu sorun
/// yok — kullanıcı geri bildirimiyle doğrulandı: "havadan kaydettiğim seslerin ... üst üste
/// iki kat gibi"). Pencereyi kısa tutmak bu birikimi sınırlı tutuyor; tam düzeltme için yerel/
/// uyarlanır saat kurtarma (drift telafisi) gerekir — kapsam dışı bırakıldı.
const EYE_WINDOW_SECONDS: f64 = 0.4;

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
    /// `frames_this_pass`: BU geçişin (baştan itibaren, `skip_frames`'ten önce) bulduğu toplam
    /// frame sayısı — anchor'ı önbelleğe alıp almamaya karar vermek için (bkz. `DecodeMsg::Done`
    /// işleyicisi: 0 ise anchor gerçek bir senkron kilidi değil, önbelleğe ALINMAMALI).
    Done { anchor: f64, frames_this_pass: usize },
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
    /// En son CRC-doğrulanmış çağrı işareti/metin — log satırları sadece frame metadatası
    /// gösterdiğinden (ham metni değil), bunlar log panelinde ayrıca/belirgin gösteriliyor.
    last_callsign: Option<String>,
    last_text: Option<String>,

    // Canlı decode.
    live: Option<LiveCapture>,
    last_live_decode: Instant,
    /// Şu ana kadar UI'a uygulanmış frame sayısı — `wav_sim_frames_applied` ile aynı amaç
    /// (bkz. `maybe_kick_live_decode`).
    live_frames_applied: usize,
    /// İlk pas'ta bulunan senkron anchor'ı — sonraki pas'larda yeniden ARAMA yapmadan doğrudan
    /// bu kullanılır (bkz. `maybe_kick_live_decode` dokümantasyonu — pas'tan pas'a arama
    /// aralığı değiştiğinden yeniden arama farklı bir anchor bulup tüm yürüyüşü bozabiliyordu).
    live_cached_anchor: Option<f64>,

    // Bir WAV dosyasını gerçek zamanlı hızda "oynatıyormuş gibi" besleme (anlık toplu decode
    // yerine — hem tile'ların gerçek KG-STV hızında gelmesi hem göz diyagramının o ana kadar
    // gelen sesi göstermesi için).
    wav_sim: Option<kgstv_modem::PlaybackSim>,
    last_wav_sim_decode: Instant,
    /// Şu ana kadar UI'a uygulanmış (log'a eklenmiş/tile'ı çizilmiş) frame sayısı — bir sonraki
    /// pas'ın hangi frame'leri sessizce atlayıp SADECE yenilerini uygulayacağını belirler
    /// (bkz. `maybe_kick_wav_sim_decode`).
    wav_sim_frames_applied: usize,
    /// `live_cached_anchor` ile aynı amaç, gerçek zamanlı WAV oynatma simülasyonu için.
    wav_sim_cached_anchor: Option<f64>,
    /// Oynatma tamamen bitince yapılan son (bütün veriyi güvenlik payı olmadan kapsayan)
    /// decode pas'ından sonra `true` — artık her 700ms'de bir yeniden decode denemesini durdurur.
    wav_sim_final_pass_done: bool,

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
            last_callsign: None,
            last_text: None,
            live: None,
            last_live_decode: Instant::now(),
            live_frames_applied: 0,
            live_cached_anchor: None,
            wav_sim: None,
            last_wav_sim_decode: Instant::now(),
            wav_sim_frames_applied: 0,
            wav_sim_cached_anchor: None,
            wav_sim_final_pass_done: false,
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
        self.last_callsign = None;
        self.last_text = None;
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
        self.wav_sim = None; // ve gerçek zamanlı oynatma simülasyonunu
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
            run_decode(&ctx1, &tx, path_str, audio.sample_rate, samples, 0, None);
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
                self.wav_sim = None; // canlı mod gerçek zamanlı WAV oynatmayı durdurur
                self.live = Some(cap);
                self.live_frames_applied = 0;
                self.live_cached_anchor = None;
                self.last_live_decode = Instant::now() - LIVE_REDECODE_INTERVAL; // hemen tetikle
                let _ = ctx;
            }
            Err(e) => {
                self.status = format!("Canlı yakalama başlatılamadı: {e}");
            }
        }
    }

    /// Bir WAV dosyasını, anlık toplu decode yerine, gerçek çalma süresine eşit sürede
    /// "oynatıyormuş gibi" beslemeye başlar (bkz. `kgstv_modem::PlaybackSim`) — hem tile'ların
    /// gerçek KG-STV hızında ekrana gelmesi hem göz diyagramının sadece o ana kadar gelen
    /// sesi gösterebilmesi için.
    fn start_wav_sim(&mut self, path: PathBuf) {
        match kgstv_modem::load_mono(&path) {
            Ok(audio) => {
                let name = path.display().to_string();
                self.status = format!(
                    "Gerçek zamanlı oynatılıyor: {name} ({:.1} sn)",
                    audio.samples.len() as f64 / audio.sample_rate
                );
                self.current_file = Some(name);
                self.reset_decode_state();
                self.live = None; // gerçek zamanlı oynatma canlı mikrofon dinlemeyi durdurur
                self.last_anchor = None;
                self.wav_sim = Some(kgstv_modem::PlaybackSim::start(audio.samples, audio.sample_rate));
                self.wav_sim_frames_applied = 0;
                self.wav_sim_cached_anchor = None;
                self.wav_sim_final_pass_done = false;
                self.last_wav_sim_decode = Instant::now() - WAV_SIM_REDECODE_INTERVAL; // hemen tetikle
            }
            Err(e) => {
                self.status = format!("WAV okunamadı: {e}");
            }
        }
    }

    /// Canlı modda periyodik olarak biriken tamponu yeniden decode etmeyi dener (önceki decode
    /// bitmişse). `decode_wav_sequential_cb`'nin durum tutan bir API'si olmadığından her pas
    /// tamponu baştan taramak ZORUNDA, ama bunu `maybe_kick_wav_sim_decode`'daki gibi
    /// UI'a "sıfırdan başlıyor" gibi göstermeden (görüntü/log silinmeden, sadece önceki
    /// pas'larda görülmemiş YENİ frame'ler uygulanarak) yapıyoruz — aksi hâlde her ~2sn'de bir
    /// ekran karardığı/frame sayacı 0'a döndüğü izlenimi veriyordu (kullanıcı geri bildirimi).
    ///
    /// **İstisna:** `LiveCapture`'ın halka tamponu `LIVE_BUFFER_SECONDS` dolunca eski örnekleri
    /// atmaya başlar — o noktadan sonra "frame index N" artık aynı sesi göstermeyebileceğinden
    /// (tamponun başı kaymış olur), güvenli olmak için o pas'ta TAM sıfırlama yapılır.
    fn maybe_kick_live_decode(&mut self, ctx: &egui::Context) {
        let Some(live) = &self.live else { return };
        if self.decoding || self.last_live_decode.elapsed() < LIVE_REDECODE_INTERVAL {
            return;
        }
        let sample_rate = live.sample_rate;
        let raw = live.snapshot();
        // Güvenlik payı: `maybe_kick_wav_sim_decode` ile aynı gerekçe — artımlı uygulamada
        // hiçbir frame yeniden ziyaret edilmediğinden, tamponun tam ucundaki kesilmiş bir
        // frame'i kalıcı olarak yanlış çizme riskini önler.
        let margin = ((0.25 * sample_rate) as usize).min(raw.len());
        let safe_len = raw.len() - margin;
        if safe_len < 4096 {
            return; // henüz anlamlı bir decode denemesi için yeterli "güvenli" örnek yok
        }

        let buffer_capacity = (LIVE_BUFFER_SECONDS * sample_rate) as usize;
        let ring_full = raw.len() >= buffer_capacity.saturating_sub(4096);
        if ring_full {
            self.reset_decode_state();
            self.live_frames_applied = 0;
            // Tamponun başı kaymış (eski örnekler atıldı) — önbellekteki anchor artık bu yeni
            // referans çerçevesinde YANLIŞ konuma işaret eder; taze bir arama şart.
            self.live_cached_anchor = None;
        }

        self.last_live_decode = Instant::now();
        let samples = Arc::new(raw[..safe_len].to_vec());
        self.last_audio = Some((sample_rate, samples.clone()));

        self.decoding = true;
        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);
        let ctx1 = ctx.clone();
        let skip = self.live_frames_applied;
        let known_anchor = self.live_cached_anchor;
        std::thread::spawn(move || {
            run_decode(&ctx1, &tx, "(canlı)".to_string(), sample_rate, samples, skip, known_anchor);
        });
    }

    /// `maybe_kick_live_decode`'a benzer, ama İKİ önemli farkla — ikisi de kullanıcının
    /// gördüğü "sürekli başa dönüp yeniden tarıyor" sorununu çözmek için:
    ///
    /// 1. **Durum sıfırlanmaz, artımlı uygulanır**: `decode_wav_sequential_cb`'nin durumu
    ///    hatırlayan bir API'si olmadığından her pas'ta baştan yürümek ZORUNDA, ama bunu
    ///    `reset_decode_state()` ile UI'a "her seferinde temiz sayfa" gibi göstermek yerine,
    ///    önceki pas'larda zaten görülmüş (`wav_sim_frames_applied`) frame'leri `run_decode`'a
    ///    sessizce atlatıp SADECE yeni frame'leri uyguluyoruz — görüntü/log asla silinmez,
    ///    sadece büyür.
    /// 2. **Güvenlik payı**: tamponun en ucuna kadar decode etmek, o an tam ortasında kesilmiş
    ///    bir frame'i (bozuk bitlerle) yanlış çözüp KALICI olarak ekrana çizme riski taşır
    ///    (artık her pas'ta silinmediğinden, önceki davranışın aksine bu asla düzelmezdi) —
    ///    bu yüzden en son ~0.25sn'lik kısmı decode dışında bırakıyoruz.
    fn maybe_kick_wav_sim_decode(&mut self, ctx: &egui::Context) {
        let Some(sim) = &self.wav_sim else { return };
        if self.wav_sim_final_pass_done {
            return; // oynatma bitti ve son (tam) pas zaten yapıldı — artık tekrar denemeye gerek yok
        }
        if self.decoding || self.last_wav_sim_decode.elapsed() < WAV_SIM_REDECODE_INTERVAL {
            return;
        }
        let sample_rate = sim.sample_rate;
        let raw = sim.snapshot();
        let is_done = sim.is_done();
        // Oynatma bittiyse artık "hareketli bir hedef" yok — güvenlik payına gerek kalmadan
        // kalan her şeyi decode edebiliriz.
        let safety_margin = if is_done { 0 } else { ((0.25 * sample_rate) as usize).min(raw.len()) };
        let safe_len = raw.len() - safety_margin;
        if safe_len < 4096 {
            return; // henüz anlamlı bir decode denemesi için yeterli "güvenli" veri yok
        }
        self.last_wav_sim_decode = Instant::now();
        self.wav_sim_final_pass_done = is_done;
        let samples = Arc::new(raw[..safe_len].to_vec());
        let received_secs = samples.len() as f64 / sample_rate;
        let total_secs = sim.total_len() as f64 / sample_rate;
        self.last_audio = Some((sample_rate, samples.clone()));

        self.decoding = true;
        let (tx, rx): (Sender<DecodeMsg>, Receiver<DecodeMsg>) = mpsc::channel();
        self.rx = Some(rx);
        let ctx1 = ctx.clone();
        let label = format!("(gerçek zamanlı, {received_secs:.1}/{total_secs:.1} sn)");
        let skip = self.wav_sim_frames_applied;
        // `PlaybackSim`'in tamponu (canlı mikrofonun aksine) asla eskiyi atmıyor, sadece
        // büyüyor — bulunan anchor oturum boyunca hep geçerli kalır.
        let known_anchor = self.wav_sim_cached_anchor;
        std::thread::spawn(move || {
            run_decode(&ctx1, &tx, label, sample_rate, samples, skip, known_anchor);
        });
    }

    /// Göz diyagramını yeniden hesaplar. **Kritik:** faz katlaması gerçek senkron `anchor`'ına
    /// göre hizalanmalı — anchor bilinmeden `(t/tb)%1` gibi keyfi bir başlangıçtan katlamak,
    /// göz diyagramının "orta noktada açık" yapısını rastgele bir faza kaydırıp düz/anlamsız
    /// bir çizim üretir (gerçek kayıtla doğrulanan bir hataydı — bkz. `docs/protocol_spec.md`).
    /// Bu yüzden `last_anchor` bilinmeden (henüz decode tamamlanmadan) çizim yapılmaz.
    ///
    /// **Sinyal seçimi:** `ToneClock::soft_metric_at`'in matched-filter korelasyonu (ilk
    /// denenen yaklaşım) sembol içinde yumuşak/sinüsvari bir eğri üretiyordu — klasik osiloskop
    /// göz diyagramının beklenen "düz plato + keskin geçiş" görünümü DEĞİL (kullanıcı geri
    /// bildirimi ve bir referans görselle doğrulandı). Bunun yerine `kgstv_modem::
    /// instantaneous_freq` (anlık frekans discriminator'ı) kullanılıyor — sembol içinde frekans
    /// gerçekten düz kaldığından (CPFSK'nin doğası gereği) ve sadece sembol sınırlarında
    /// değiştiğinden, bu tam olarak klasik göz diyagramı şeklini üretiyor. Frekans, `±300Hz`
    /// (MSK'nin nominal sapması) ile [-1,1]'e normalize edilir — 4L-FSK'nin ara seviyeleri
    /// (1400/1600Hz) de doğal olarak ±0.33'e denk gelir.
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
        let tb = sr / kgstv_modem::BAUD;
        if tb < 1.0 {
            self.eye_points.clear();
            return;
        }
        const CENTER_FREQ: f64 = 1500.0;
        const LP_CUTOFF: f64 = 500.0;
        const DEVIATION: f64 = 300.0; // MSK'nin nominal sapması — normalizasyon ölçeği
        let freq = kgstv_modem::instantaneous_freq(&filtered, sr, CENTER_FREQ, LP_CUTOFF);

        let mut points = Vec::with_capacity(freq.len());
        for (i, &f) in freq.iter().enumerate() {
            let m = (((f - CENTER_FREQ) / DEVIATION) as f32).clamp(-1.3, 1.3);
            let abs_t = start as f64 + i as f64;
            let phase = ((abs_t - anchor) / tb).rem_euclid(1.0) as f32;
            points.push((phase, m));
        }

        // Otomatik ortalama: `anchor`'da MSK'nin kendi (incoherent) bit kararı için önemsiz
        // ama göz diyagramının fazını kaydırabilen küçük bir sistematik ofset olabilir
        // (kullanıcı geri bildirimi: "eye diyagram ... tam ortalanmamış sağlı sollu"). Gözün
        // en AÇIK (frekansın en durağan/en düşük varyanslı) noktasını bulup onu faz=0.5'e
        // hizalayarak diyagram, anchor'daki küçük hatalardan bağımsız her zaman ortalanmış
        // görünüyor.
        if !points.is_empty() {
            const N_BINS: usize = 24;
            let mut sum = [0.0f64; N_BINS];
            let mut sum_sq = [0.0f64; N_BINS];
            let mut count = [0usize; N_BINS];
            for &(phase, m) in &points {
                let idx = ((phase * N_BINS as f32) as usize).min(N_BINS - 1);
                sum[idx] += m as f64;
                sum_sq[idx] += (m as f64) * (m as f64);
                count[idx] += 1;
            }
            let mut best_bin = None;
            let mut best_var = f64::INFINITY;
            for i in 0..N_BINS {
                if count[i] < 4 {
                    continue;
                }
                let mean = sum[i] / count[i] as f64;
                let var = sum_sq[i] / count[i] as f64 - mean * mean;
                if var < best_var {
                    best_var = var;
                    best_bin = Some(i);
                }
            }
            if let Some(bin) = best_bin {
                let best_phase = (bin as f32 + 0.5) / N_BINS as f32;
                let shift = 0.5 - best_phase;
                for p in points.iter_mut() {
                    p.0 = (p.0 + shift).rem_euclid(1.0);
                }
            }
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
                    // Log satırı sadece frame metadatasını (x/y/sc/boyut) gösteriyor — çağrı
                    // işareti/metin frame'lerinin GERÇEK içeriğini (ham metnini) ayrıca decode
                    // edip hem log'a hem üstteki belirgin özet alanına yazıyoruz (aksi hâlde
                    // "Some(Callsign)" görünüp içerik hiç ortaya çıkmıyordu — kullanıcı geri
                    // bildirimi: "cagri isaretini cozemiyor").
                    if fr.data_crc_ok {
                        if let Some(data_bits) = &fr.data_bits {
                            let text = bits_to_ascii(data_bits);
                            match cmd {
                                Some(Command::Callsign) => {
                                    self.log_lines.push(format!("      → ÇAĞRI İŞARETİ: {text}"));
                                    self.last_callsign = Some(text);
                                }
                                Some(Command::Text) => {
                                    self.log_lines.push(format!("      → METİN: {text}"));
                                    self.last_text = Some(text);
                                }
                                _ => {}
                            }
                        }
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
                DecodeMsg::Done { anchor, frames_this_pass } => {
                    self.decoding = false;
                    self.last_anchor = Some(anchor);
                    // Anchor'ı SADECE bu geçiş gerçekten en az bir frame bulduysa önbelleğe al
                    // (yani gerçek bir senkron kilidiydi) — aksi hâlde `find_sync`'in "en iyi
                    // eldeki" (ama aslında GEÇERSİZ/gürültü) sonucunu önbelleğe alıp sonsuza
                    // dek yeniden kullanmış oluruz, bu da GERÇEK bir sinyal gelse bile asla
                    // senkronlanamamaya yol açar (kullanıcı geri bildirimiyle bulunan bir
                    // hataydı: "cagri isareti cozmedi ... goruntu hic cozumlemedi"). 0 ise
                    // önbellek temizlenir ki bir sonraki pas taze bir arama yapsın.
                    if self.wav_sim.is_some() {
                        // Bir sonraki pas bu kadarını sessizce atlayıp sadece yeni frame'leri
                        // uygulasın (bkz. `maybe_kick_wav_sim_decode`).
                        self.wav_sim_frames_applied = self.frames_total;
                        self.wav_sim_cached_anchor = if frames_this_pass > 0 { Some(anchor) } else { None };
                    } else if self.live.is_some() {
                        self.live_frames_applied = self.frames_total;
                        self.live_cached_anchor = if frames_this_pass > 0 { Some(anchor) } else { None };
                    }
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

/// Ortak decode çalıştırıcısı: dosya modu, canlı mod ve gerçek-zamanlı WAV oynatma simülasyonu
/// tarafından, zaten arka planda çalışan bir thread içinden çağrılır. Senkron bulur, frame
/// frame yürür, her frame'i `tx` üzerinden UI thread'ine bildirir.
///
/// `skip_frames`: yürüyüşün baştan `skip_frames` kadarını (zaten önceki bir pas'ta UI'a
/// uygulanmış) SESSİZCE atlar, sadece YENİ frame'leri bildirir. Gerçek-zamanlı WAV oynatma
/// simülasyonu için — tampon her pas'ta büyüdüğünden yine baştan yürümek zorunda (durum tutan
/// bir API yok), ama bunu UI'a her seferinde "en baştan" gibi göstermek (görüntü/log'u
/// sıfırlayıp yeniden doldurmak) kullanıcıya sanki her pas'ta "başa dönüp yeniden tarıyormuş"
/// gibi görünür. Dosya modunda `skip_frames=0`.
///
/// `known_anchor`: verilirse senkron ARAMASI tamamen atlanır, doğrudan bu anchor kullanılır.
/// Büyüyen bir tamponu tekrar tekrar decode eden çağıranlar (canlı mikrofon/gerçek-zamanlı WAV
/// oynatma) için KRİTİK — her pas'ta yeniden arama yapmak, arama aralığı tamponla birlikte
/// değiştiğinden pas'tan pas'a birkaç örnek farklı bir anchor bulabiliyordu, bu da `skip_frames`
/// ile eşleştirilmiş "önceden decode edilmiş" frame varsayımını bozup tüm yürüyüşün erken
/// durmasına yol açıyordu (bkz. `kgstv_modem::decode_wav_sequential_cb_from_anchor`
/// dokümantasyonu — kullanıcı geri bildirimiyle bulunan bir hataydı).
fn run_decode(
    ctx: &egui::Context,
    tx: &Sender<DecodeMsg>,
    label: String,
    sample_rate: f64,
    samples: Arc<Vec<f64>>,
    skip_frames: usize,
    known_anchor: Option<f64>,
) {
    let _ = tx.send(DecodeMsg::Started { label, sample_rate, n_samples: samples.len() });
    ctx.request_repaint();

    let tx2 = tx.clone();
    let ctx2 = ctx.clone();
    // `Arc<AtomicUsize>` (basit `let mut seen` DEĞİL): closure `decode_wav_sequential_cb*`'ye
    // TAŞINDIĞI için, çağrıdan sonra son sayıyı okuyabilmemiz lazım — paylaşımlı bir sayaç şart.
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_cb = seen.clone();
    let frame_cb = move |fr| {
        let n = seen_cb.fetch_add(1, Ordering::Relaxed);
        if n >= skip_frames {
            let _ = tx2.send(DecodeMsg::Frame(fr));
            ctx2.request_repaint();
        }
    };

    let anchor = if let Some(a) = known_anchor {
        kgstv_modem::decode_wav_sequential_cb_from_anchor(&samples, sample_rate, a, 5000, frame_cb);
        a
    } else {
        // Senkronun kayıt cihazı başlatma gecikmesi/sessizlik yüzünden ilk saniyelerde
        // olmayabileceğini göz önünde bulundurarak ilk 15 saniyeyi tara.
        let coarse_search_len = ((15.0 * sample_rate) as i64).min(samples.len() as i64);
        kgstv_modem::decode_wav_sequential_cb(&samples, sample_rate, coarse_search_len, 5000, frame_cb)
    };
    let frames_this_pass = seen.load(Ordering::Relaxed);
    let _ = tx.send(DecodeMsg::Done { anchor, frames_this_pass });
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

/// MSB-first bit dizisini ASCII metne çevirir (`ascii_to_bits`'in tersi) — çağrı işareti/metin
/// frame'lerinin ham verisini okunabilir hâle getirmek için. Yazdırılamayan baytlar `.` ile
/// gösterilir (Shift-JIS gibi çok baytlı kodlamalar olası ama şimdilik düz ASCII varsayılıyor).
fn bits_to_ascii(bits: &[u8]) -> String {
    bits.chunks(8)
        .filter(|c| c.len() == 8)
        .map(|c| {
            let byte = c.iter().fold(0u8, |acc, &b| (acc << 1) | (b & 1));
            if byte.is_ascii_graphic() || byte == b' ' {
                byte as char
            } else {
                '.'
            }
        })
        .collect()
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

    // DÜŞÜK alfa kritik: osiloskop "fosfor kalıcılığı" görünümü, YÜZLERCE üst üste binen ize
    // dayanır — her iz görece SAYDAM olmalı ki yoğunluk kademeli birikip net bir gradyan
    // oluştursun (sık üst üste binen düz platolar parlak/opak, seyrek geçiş/kesişim bölgeleri
    // soluk kalsın). Yüksek bir alfa (ör. %43) birkaç izden sonra her yeri opak yeşile
    // doyurup TÜM yoğunluk bilgisini (dolayısıyla kesişim noktalarını) yok ediyordu — kullanıcı
    // geri bildirimiyle bulunan asıl sorun buydu.
    let stroke = egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(80, 220, 120, 20));
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
        self.maybe_kick_wav_sim_decode(&ctx);
        self.recompute_eye_diagram();
        if self.live.is_some() || self.wav_sim.is_some() || self.tx_encoding || self.tx_playing {
            // Canlı/gerçek-zamanlı-oynatma/kodlama/çalma sırasında düzenli aralıklarla yeniden
            // çizim tetikle.
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
                        let busy = !self.decoding && self.live.is_none() && self.wav_sim.is_none();
                        if ui.add_enabled(busy, egui::Button::new("📂 WAV Aç...")).clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                                self.open_file(&ctx, path);
                            }
                        }
                        if self.wav_sim.is_some() {
                            if ui.button("⏹ Gerçek Zamanlı Durdur").clicked() {
                                self.wav_sim = None;
                                self.status = "Gerçek zamanlı oynatma durduruldu.".to_string();
                            }
                        } else if ui
                            .add_enabled(!self.decoding && self.live.is_none(), egui::Button::new("🐢 Gerçek Zamanlı Oynat"))
                            .clicked()
                        {
                            if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav"]).pick_file() {
                                self.start_wav_sim(path);
                            }
                        }
                        let live_label =
                            if self.live.is_some() { "⏹ Canlı Decode Durdur" } else { "🎙 Canlı Decode Başlat" };
                        if ui.add_enabled(self.wav_sim.is_none(), egui::Button::new(live_label)).clicked() {
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
                    ui.label(
                        egui::RichText::new(format!(
                            "📻 Çağrı işareti: {}",
                            self.last_callsign.as_deref().unwrap_or("(henüz yok)")
                        ))
                        .strong(),
                    );
                    if let Some(text) = &self.last_text {
                        ui.label(egui::RichText::new(format!("💬 Metin: {text}")).strong());
                    }
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

        egui::Panel::bottom("eye").resizable(true).default_size(340.0).min_size(160.0).show(ui, |ui| {
            ui.heading("Göz Diyagramı (Eye Diagram) — anlık frekans discriminator");
            let (rect, _resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ui.available_height()),
                egui::Sense::hover(),
            );
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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_title("Atölye m-KGSTV"),
        ..Default::default()
    };
    // İsteğe bağlı: `kgstv-gui <kayit.wav>` şeklinde çağrılırsa o dosyayı açılışta otomatik
    // yükler (native dosya seçiciyle uğraşmadan hızlı test/betikleme için kullanışlı).
    let cli_wav_path = std::env::args().nth(1).map(PathBuf::from);
    eframe::run_native(
        "Atölye m-KGSTV",
        options,
        Box::new(move |cc| {
            let mut app = KgstvApp::default();
            if let Some(path) = cli_wav_path {
                app.open_file(&cc.egui_ctx, path);
            }
            Ok(Box::new(app))
        }),
    )
}
