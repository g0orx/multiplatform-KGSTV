//! Canlı ses yakalama (RX) ve çalma (TX): varsayılan giriş/çıkış cihazlarıyla `cpal`
//! üzerinden — Windows (WASAPI), Linux (ALSA), macOS (CoreAudio). Giriş tarafı `wav.rs` ile
//! aynı `f64` mono örnek formatını üretir, böylece aynı `session::decode_wav_*` fonksiyonları
//! hem dosyadan hem canlı akıştan beslenebilir. Çıkış tarafı, `session::encode_session_audio`
//! ile üretilmiş bir modüle edilmiş sinyali doğrudan hoparlöre/ses kartına çalar.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Sürekli büyüyen ama belirli bir süreyi aşınca baştan kırpılan (ring-buffer benzeri) örnek
/// tamponu. `max_samples` aşılınca en eski örnekler atılır — bellek sınırsız büyümesin diye.
struct RingBuffer {
    data: VecDeque<f64>,
    max_samples: usize,
}

impl RingBuffer {
    fn push_slice(&mut self, samples: &[f64]) {
        self.data.extend(samples.iter().copied());
        while self.data.len() > self.max_samples {
            self.data.pop_front();
        }
    }
}

/// Aktif bir canlı yakalama oturumu. `Drop` edildiğinde akış durur.
pub struct LiveCapture {
    pub sample_rate: f64,
    buffer: Arc<Mutex<RingBuffer>>,
    _stream: cpal::Stream,
}

impl LiveCapture {
    /// Varsayılan giriş cihazından yakalamayı başlatır. `max_seconds`: tamponda tutulacak
    /// azami süre (daha eski örnekler otomatik atılır).
    pub fn start(max_seconds: f64) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "Varsayılan ses giriş cihazı bulunamadı".to_string())?;
        let config = device
            .default_input_config()
            .map_err(|e| format!("Giriş yapılandırması alınamadı: {e}"))?;

        let sample_rate = config.sample_rate() as f64;
        let channels = config.channels() as usize;
        let max_samples = (max_seconds * sample_rate).round() as usize;
        let buffer = Arc::new(Mutex::new(RingBuffer { data: VecDeque::new(), max_samples }));
        let buffer_cb = buffer.clone();

        let err_fn = |e| eprintln!("[kgstv-modem::live] ses akışı hatası: {e}");
        let stream_config: cpal::StreamConfig = config.clone().into();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[f32], _| {
                        push_downmixed(&buffer_cb, data, channels, |v| v as f64);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Giriş akışı kurulamadı: {e}"))?,
            cpal::SampleFormat::I16 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[i16], _| {
                        push_downmixed(&buffer_cb, data, channels, |v| v as f64 / i16::MAX as f64);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| format!("Giriş akışı kurulamadı: {e}"))?,
            cpal::SampleFormat::U16 => device
                .build_input_stream(
                    stream_config.clone(),
                    move |data: &[u16], _| {
                        push_downmixed(&buffer_cb, data, channels, |v| {
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

        Ok(LiveCapture { sample_rate, buffer, _stream: stream })
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

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Verilen mono örnekleri (-1.0..1.0, `sample_rate` Hz) varsayılan ses çıkış cihazından çalar,
/// çalma bitene kadar (çağıran thread'i) bloklar. `session::encode_session_audio` ile üretilen
/// TX sinyalini doğrudan hoparlöre/ses kartına vermek için — çağıran, UI donmasın diye bunu
/// ayrı bir thread'den çağırmalıdır.
pub fn play_blocking(samples: &[f64], sample_rate: f64) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "Varsayılan ses çıkış cihazı bulunamadı".to_string())?;

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
    data: &[T],
    channels: usize,
    to_f64: impl Fn(T) -> f64,
) {
    let mono: Vec<f64> = if channels > 1 {
        data.chunks(channels)
            .map(|c| c.iter().map(|&v| to_f64(v)).sum::<f64>() / channels as f64)
            .collect()
    } else {
        data.iter().map(|&v| to_f64(v)).collect()
    };
    if let Ok(mut buf) = buffer.lock() {
        buf.push_slice(&mono);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
