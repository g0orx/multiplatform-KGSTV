//! Canlı ses yakalama (RX) ve çalma (TX): varsayılan giriş/çıkış cihazlarıyla `cpal`
//! üzerinden — Windows (WASAPI), Linux (ALSA), macOS (CoreAudio). Giriş tarafı `wav.rs` ile
//! aynı `f64` mono örnek formatını üretir, böylece aynı `session::decode_wav_*` fonksiyonları
//! hem dosyadan hem canlı akıştan beslenebilir. Çıkış tarafı, `session::encode_session_audio`
//! ile üretilmiş bir modüle edilmiş sinyali doğrudan hoparlöre/ses kartına çalar.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Sürekli büyüyen ama belirli bir süreyi aşınca baştan kırpılan (ring-buffer benzeri) örnek
/// tamponu. `max_samples` aşılınca en eski örnekler atılır — bellek sınırsız büyümesin diye.
struct RingBuffer {
    data: VecDeque<f64>,
    max_samples: usize,
    /// `data[0]`'ın MUTLAK (tampon hiç kırpılmamış gibi, oturumun başından sayılan) örnek
    /// indeksi — yani şimdiye kadar öne'den atılan toplam örnek sayısı. Tampon dolup eski
    /// örnekler atılmaya başladığında, `kgstv-gui`'nin önbelleğe aldığı bir anchor'ın (bir
    /// önceki `snapshot()`'a göre hesaplanmış örnek konumu) YENİ bir `snapshot()`'a göre
    /// KAYMIŞ olacağını fark edip düzeltebilmesi için gerekli (bkz. `kgstv-gui`
    /// `maybe_kick_live_decode` dokümantasyonu — kullanıcı geri bildirimi: "ilk seferinde
    /// duzgun calisiyor ama sonra ... tekrar tum frameleri almaya calisti").
    front_absolute_index: u64,
}

impl RingBuffer {
    fn push_slice(&mut self, samples: &[f64]) {
        self.data.extend(samples.iter().copied());
        while self.data.len() > self.max_samples {
            self.data.pop_front();
            self.front_absolute_index += 1;
        }
    }
}

/// Sistemdeki ses giriş cihazlarının adlarını listeler (dropdown doldurmak için).
/// (cpal 0.18: cihaz adı `Device::name()` değil `Display`/`to_string()` üzerinden gelir.)
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(it) => it.map(|d| d.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Sistemdeki ses çıkış cihazlarının adlarını listeler (dropdown doldurmak için).
pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(it) => it.map(|d| d.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Varsayılan giriş cihazının adı (dropdown'da "hangisi seçili olsun" için).
pub fn default_input_device_name() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| d.to_string())
}

/// Varsayılan çıkış cihazının adı.
pub fn default_output_device_name() -> Option<String> {
    cpal::default_host().default_output_device().map(|d| d.to_string())
}

fn find_input_device(name: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match name {
        Some(n) => host
            .input_devices()
            .map_err(|e| format!("Giriş cihazları listelenemedi: {e}"))?
            .find(|d| d.to_string() == n)
            .ok_or_else(|| format!("Giriş cihazı bulunamadı: {n}")),
        None => host.default_input_device().ok_or_else(|| "Varsayılan ses giriş cihazı bulunamadı".to_string()),
    }
}

fn find_output_device(name: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match name {
        Some(n) => host
            .output_devices()
            .map_err(|e| format!("Çıkış cihazları listelenemedi: {e}"))?
            .find(|d| d.to_string() == n)
            .ok_or_else(|| format!("Çıkış cihazı bulunamadı: {n}")),
        None => host.default_output_device().ok_or_else(|| "Varsayılan ses çıkış cihazı bulunamadı".to_string()),
    }
}

/// Aktif bir canlı yakalama oturumu. `Drop` edildiğinde akış durur.
pub struct LiveCapture {
    pub sample_rate: f64,
    buffer: Arc<Mutex<RingBuffer>>,
    gain: Arc<Mutex<f32>>,
    /// En son yakalanan parçanın tepe (peak) genliği (0.0'dan yukarı, tipik olarak ~0-1.5) —
    /// GUI'nin VU metresi bunu periyodik olarak okur.
    level: Arc<Mutex<f32>>,
    _stream: cpal::Stream,
}

impl LiveCapture {
    /// Belirtilen (ya da `None` ise varsayılan) giriş cihazından yakalamayı başlatır.
    /// `max_seconds`: tamponda tutulacak azami süre (daha eski örnekler otomatik atılır).
    /// `initial_gain`: örneklere uygulanan çarpan (1.0 = değişiklik yok) — sonradan
    /// `set_gain()` ile canlı olarak değiştirilebilir.
    pub fn start(max_seconds: f64, device_name: Option<&str>, initial_gain: f32) -> Result<Self, String> {
        let device = find_input_device(device_name)?;
        let config = device
            .default_input_config()
            .map_err(|e| format!("Giriş yapılandırması alınamadı: {e}"))?;

        let sample_rate = config.sample_rate() as f64;
        let channels = config.channels() as usize;
        let max_samples = (max_seconds * sample_rate).round() as usize;
        let buffer = Arc::new(Mutex::new(RingBuffer { data: VecDeque::new(), max_samples, front_absolute_index: 0 }));
        let buffer_cb = buffer.clone();
        let gain = Arc::new(Mutex::new(initial_gain));
        let gain_cb = gain.clone();
        let level = Arc::new(Mutex::new(0.0f32));
        let level_cb = level.clone();

        let err_fn = |e| eprintln!("[kgstv-modem::live] ses akışı hatası: {e}");
        let stream_config: cpal::StreamConfig = config.clone().into();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[f32], _| {
                        push_downmixed(&buffer_cb, &gain_cb, &level_cb, data, channels, |v| v as f64);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Giriş akışı kurulamadı: {e}"))?,
            cpal::SampleFormat::I16 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[i16], _| {
                        push_downmixed(&buffer_cb, &gain_cb, &level_cb, data, channels, |v| {
                            v as f64 / i16::MAX as f64
                        });
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Giriş akışı kurulamadı: {e}"))?,
            cpal::SampleFormat::U16 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[u16], _| {
                        push_downmixed(&buffer_cb, &gain_cb, &level_cb, data, channels, |v| {
                            (v as f64 - 32768.0) / 32768.0
                        });
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Giriş akışı kurulamadı: {e}"))?,
            fmt => return Err(format!("Desteklenmeyen örnek formatı: {fmt:?}")),
        };

        stream.play().map_err(|e| format!("Ses akışı başlatılamadı: {e}"))?;

        Ok(LiveCapture { sample_rate, buffer, gain, level, _stream: stream })
    }

    /// O ana kadar biriken örneklerin bir kopyasını döner (decode için).
    pub fn snapshot(&self) -> Vec<f64> {
        let buf = self.buffer.lock().unwrap();
        buf.data.iter().copied().collect()
    }

    /// Şu ana kadar biriken örnek sayısı.
    pub fn len(&self) -> usize {
        self.buffer.lock().unwrap().data.len()
    }

    /// `snapshot()`'ın döneceği dizinin İLK örneğinin MUTLAK (oturumun başından sayılan)
    /// indeksi — tampon henüz hiç kırpılmadıysa 0, kırpılmaya başladıysa şimdiye kadar
    /// atılan örnek sayısı. Önceden önbelleğe alınmış bir anchor'ın (eski bir `snapshot()`'a
    /// göre hesaplanmıştı) yeni bir `snapshot()`'a göre ne kadar KAYDIĞINI hesaplamak için
    /// kullanılır (bkz. `kgstv-gui` `maybe_kick_live_decode`).
    pub fn front_absolute_index(&self) -> u64 {
        self.buffer.lock().unwrap().front_absolute_index
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Giriş kazancını (çarpan; 1.0 = değişiklik yok) CANLI olarak günceller — akışı yeniden
    /// başlatmaya gerek yok.
    pub fn set_gain(&self, gain: f32) {
        *self.gain.lock().unwrap() = gain.max(0.0);
    }

    pub fn gain(&self) -> f32 {
        *self.gain.lock().unwrap()
    }

    /// En son yakalanan parçanın tepe (peak) genliği — VU metresi için (GUI periyodik okur).
    pub fn level(&self) -> f32 {
        *self.level.lock().unwrap()
    }
}

/// Zaten yüklenmiş bir WAV'ın örneklerini GERÇEK ZAMANLI hızda (parça parça, `sleep` ile
/// gerçek çalma süresine eşit toplam sürede) paylaşımlı bir tampona besleyen simülatör.
/// `LiveCapture` ile aynı arayüz şekli (`sample_rate`, `snapshot()`, `len()`) — çağıran taraf
/// (`kgstv-gui`), gerçek mikrofon dinleme ile bir WAV dosyasını "canlıymış gibi oynatma"yı
/// aynı ilerlemeli decode/göz-diyagramı koduyla ele alabilsin diye. Anlık (tüm dosyayı bir
/// kerede işleyen) decode'un aksine, göz diyagramının SADECE "o ana kadar gelen" sesi
/// görebilmesini ve tile'ların gerçek KG-STV hızında ekrana gelmesini sağlar.
pub struct PlaybackSim {
    pub sample_rate: f64,
    total_len: usize,
    buffer: Arc<Mutex<Vec<f64>>>,
    done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl PlaybackSim {
    /// `samples`'i `sample_rate` hızında, ~100ms'lik parçalar hâlinde beslemeye başlar
    /// (arka plan thread'inde) ve hemen döner.
    pub fn start(samples: Vec<f64>, sample_rate: f64) -> Self {
        let total_len = samples.len();
        let buffer = Arc::new(Mutex::new(Vec::with_capacity(total_len)));
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let buf_cb = buffer.clone();
        let done_cb = done.clone();
        let stop_cb = stop.clone();
        std::thread::spawn(move || {
            let chunk_len = ((sample_rate * 0.1) as usize).max(1); // ~100ms'lik parçalar
            let mut i = 0;
            while i < samples.len() {
                if stop_cb.load(Ordering::Relaxed) {
                    return;
                }
                let end = (i + chunk_len).min(samples.len());
                if let Ok(mut buf) = buf_cb.lock() {
                    buf.extend_from_slice(&samples[i..end]);
                }
                i = end;
                std::thread::sleep(Duration::from_secs_f64(chunk_len as f64 / sample_rate));
            }
            done_cb.store(true, Ordering::Relaxed);
        });

        PlaybackSim { sample_rate, total_len, buffer, done, stop }
    }

    /// O ana kadar "gelmiş" örneklerin bir kopyası.
    pub fn snapshot(&self) -> Vec<f64> {
        self.buffer.lock().unwrap().clone()
    }

    /// O ana kadar gelen örnek sayısı.
    pub fn len(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Dosyanın tamamı beslendi mi (oynatma bitti mi).
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// Toplam örnek sayısı (dosyanın tamamı) — ilerleme göstermek için.
    pub fn total_len(&self) -> usize {
        self.total_len
    }
}

impl Drop for PlaybackSim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Verilen mono örnekleri (-1.0..1.0, `sample_rate` Hz) belirtilen (ya da `None` ise
/// varsayılan) ses çıkış cihazından çalar, çalma bitene kadar (çağıran thread'i) bloklar.
/// `gain`: çalmadan önce örneklere uygulanan çarpan (1.0 = değişiklik yok). `session::
/// encode_session_audio` ile üretilen TX sinyalini doğrudan hoparlöre/ses kartına vermek için
/// — çağıran, UI donmasın diye bunu ayrı bir thread'den çağırmalıdır.
pub fn play_blocking(samples: &[f64], sample_rate: f64, device_name: Option<&str>, gain: f32) -> Result<(), String> {
    let device = find_output_device(device_name)?;
    let gained: Vec<f64> = if (gain - 1.0).abs() < 1e-6 {
        samples.to_vec()
    } else {
        samples.iter().map(|&v| v * gain as f64).collect()
    };
    let samples = gained.as_slice();

    let want_sr = sample_rate.round() as u32;
    let ranges: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| format!("Çıkış yapılandırmaları alınamadı: {e}"))?
        .collect();

    // Birçok gerçek çıkış cihazı (ör. dahili hoparlörler) SADECE kendi sabit örnekleme hızını
    // (genelde 48000 Hz) destekler, 44100 Hz'i değil. İstenen hız desteklenmiyorsa cihazın
    // varsayılan yapılandırmasına düş ve sesi o hıza yeniden örnekle (basit doğrusal
    // interpolasyon — TX sinyalinin bant genişliği düşük olduğundan (500-2500Hz) pratikte
    // kayıpsız sayılabilir).
    let exact = ranges.iter().find(|r| r.min_sample_rate() <= want_sr && want_sr <= r.max_sample_rate());
    let (supported, playback_samples): (cpal::SupportedStreamConfig, Vec<f64>) = match exact {
        Some(range) => (range.clone().with_sample_rate(want_sr), samples.to_vec()),
        None => {
            let default_cfg = device
                .default_output_config()
                .map_err(|e| format!("Varsayılan çıkış yapılandırması alınamadı: {e}"))?;
            let target_sr = default_cfg.sample_rate() as f64;
            (default_cfg, resample_linear(samples, sample_rate, target_sr))
        }
    };

    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let channels = config.channels as usize;
    let effective_sample_rate = config.sample_rate as f64;

    let samples = Arc::new(playback_samples);
    let total = samples.len();
    let pos = Arc::new(AtomicUsize::new(0));
    let err_fn = |e| eprintln!("[kgstv-modem::live] çıkış akışı hatası: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let samples_cb = samples.clone();
            let pos_cb = pos.clone();
            device
                .build_output_stream(
                    config.clone(),
                    move |data: &mut [f32], _| pull_upmixed(data, channels, &samples_cb, &pos_cb, |v| v as f32),
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Çıkış akışı kurulamadı: {e}"))?
        }
        cpal::SampleFormat::I16 => {
            let samples_cb = samples.clone();
            let pos_cb = pos.clone();
            device
                .build_output_stream(
                    config.clone(),
                    move |data: &mut [i16], _| {
                        pull_upmixed(data, channels, &samples_cb, &pos_cb, |v| (v * i16::MAX as f64) as i16)
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Çıkış akışı kurulamadı: {e}"))?
        }
        fmt => return Err(format!("Desteklenmeyen çıkış örnek formatı: {fmt:?}")),
    };

    stream.play().map_err(|e| format!("Çıkış akışı başlatılamadı: {e}"))?;
    // `total`/`samples`, yeniden örneklenmiş olabileceğinden süre hesabı `effective_sample_rate`
    // (cihazın gerçekten çaldığı hız) ile yapılmalı — orijinal `sample_rate` ile DEĞİL.
    let duration = Duration::from_secs_f64(total as f64 / effective_sample_rate + 0.3);
    std::thread::sleep(duration);
    Ok(())
}

/// Basit doğrusal interpolasyonla yeniden örnekleme (`from_sr` Hz -> `to_sr` Hz). Yüksek kaliteli
/// bir resampler değil, ama KG-STV'nin dar bandı (500-2500Hz) için, sadece hoparlörden çalma
/// amacıyla yeterli — WAV kaydı/decode her zaman orijinal (yeniden örneklenmemiş) hızda kalır.
fn resample_linear(samples: &[f64], from_sr: f64, to_sr: f64) -> Vec<f64> {
    if samples.is_empty() || (from_sr - to_sr).abs() < 1e-6 {
        return samples.to_vec();
    }
    let ratio = from_sr / to_sr;
    let out_len = ((samples.len() as f64) / ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let i0 = src_pos.floor() as usize;
        let frac = src_pos - i0 as f64;
        let s0 = samples.get(i0).copied().unwrap_or(0.0);
        let s1 = samples.get(i0 + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

fn pull_upmixed<T: Copy + Default>(
    data: &mut [T],
    channels: usize,
    samples: &[f64],
    pos: &AtomicUsize,
    to_sample: impl Fn(f64) -> T,
) {
    for frame in data.chunks_mut(channels) {
        let i = pos.fetch_add(1, Ordering::Relaxed);
        let v = if i < samples.len() { to_sample(samples[i]) } else { T::default() };
        for s in frame.iter_mut() {
            *s = v;
        }
    }
}

fn push_downmixed<T: Copy>(
    buffer: &Arc<Mutex<RingBuffer>>,
    gain: &Arc<Mutex<f32>>,
    level: &Arc<Mutex<f32>>,
    data: &[T],
    channels: usize,
    to_f64: impl Fn(T) -> f64,
) {
    let g = gain.lock().map(|g| *g as f64).unwrap_or(1.0);
    let mono: Vec<f64> = if channels > 1 {
        data.chunks(channels).map(|c| g * c.iter().map(|&v| to_f64(v)).sum::<f64>() / channels as f64).collect()
    } else {
        data.iter().map(|&v| g * to_f64(v)).collect()
    };
    let peak = mono.iter().fold(0.0f64, |m, &v| m.max(v.abs())) as f32;
    if let Ok(mut lv) = level.lock() {
        *lv = peak;
    }
    if let Ok(mut buf) = buffer.lock() {
        buf.push_slice(&mono);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `front_absolute_index`, tampon kapasiteyi aşıp eski örnekler atılmaya başladığında
    /// doğru artmalı — `kgstv-gui`'nin önbelleğe aldığı bir anchor'ı, tampon kırpıldıkça
    /// doğru şekilde yeniden yerelleştirebilmesi (bkz. `LiveCapture::front_absolute_index`
    /// dokümantasyonu) buna dayanıyor.
    #[test]
    fn ring_buffer_tracks_front_absolute_index_as_it_evicts() {
        let mut buf = RingBuffer { data: VecDeque::new(), max_samples: 5, front_absolute_index: 0 };

        buf.push_slice(&[1.0, 2.0, 3.0]);
        assert_eq!(buf.front_absolute_index, 0); // henüz kırpma yok (3 < kapasite 5)
        assert_eq!(buf.data.iter().copied().collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);

        buf.push_slice(&[4.0, 5.0, 6.0, 7.0]); // toplam 7 > kapasite 5 -> 2 tanesi atılır
        assert_eq!(buf.front_absolute_index, 2);
        assert_eq!(buf.data.iter().copied().collect::<Vec<_>>(), vec![3.0, 4.0, 5.0, 6.0, 7.0]);

        buf.push_slice(&[8.0]);
        assert_eq!(buf.front_absolute_index, 3);
        assert_eq!(buf.data.iter().copied().collect::<Vec<_>>(), vec![4.0, 5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn playback_sim_grows_over_time_and_finishes() {
        let sr = 4410.0; // düşük hız: testin hızlı bitmesi için (~100ms parça -> 441 örnek)
        let samples: Vec<f64> = (0..(sr as usize) / 2).map(|i| i as f64).collect(); // 0.5 sn'lik veri
        let sim = PlaybackSim::start(samples.clone(), sr);
        assert_eq!(sim.total_len(), samples.len());
        assert!(sim.is_empty());
        assert!(!sim.is_done());

        std::thread::sleep(Duration::from_millis(150));
        let mid = sim.len();
        assert!(mid > 0 && mid < samples.len(), "mid={mid} (0 ile {} arasında olmalı)", samples.len());

        std::thread::sleep(Duration::from_millis(600));
        assert!(sim.is_done());
        assert_eq!(sim.snapshot(), samples);
    }

    #[test]
    fn resample_linear_preserves_low_frequency_tone() {
        // 44100 Hz'de üretilen 1500 Hz'lik (KG-STV bandının merkezi) bir ton, 48000 Hz'e yeniden
        // örneklendiğinde hâlâ ~1500 Hz civarında, kabaca aynı genlikte olmalı (basit doğrusal
        // interpolasyonun bu dar bant için yeterli olduğunun kanıtı — bkz. `play_blocking`).
        let from_sr = 44100.0;
        let to_sr = 48000.0;
        let freq = 1500.0;
        let n = 4410; // 0.1 sn
        let input: Vec<f64> =
            (0..n).map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / from_sr).sin()).collect();
        let output = resample_linear(&input, from_sr, to_sr);

        // Süre korunmalı (± birkaç örnek).
        let expected_len = (n as f64 * to_sr / from_sr).round() as usize;
        assert!((output.len() as i64 - expected_len as i64).abs() <= 2, "len={}", output.len());

        // Çıkışta hâlâ ~1500Hz civarında salınım var mı: basit bir sıfır-geçiş sayımıyla kontrol et.
        let zero_crossings = output.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
        let expected_crossings = (2.0 * freq * (output.len() as f64 / to_sr)).round();
        let ratio = zero_crossings as f64 / expected_crossings;
        assert!((0.9..1.1).contains(&ratio), "zero_crossings={zero_crossings} expected~={expected_crossings}");
    }

    #[test]
    fn resample_linear_identity_when_rates_match() {
        let input = vec![0.1, 0.2, -0.3, 0.4];
        let output = resample_linear(&input, 44100.0, 44100.0);
        assert_eq!(output, input);
    }
}
