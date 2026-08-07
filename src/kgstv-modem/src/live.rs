//! Canlı ses yakalama: varsayılan giriş cihazından (mikrofon/soundcard) sürekli örnek
//! biriktirir. `cpal` üzerinden — Windows (WASAPI), Linux (ALSA), macOS (CoreAudio) dahil
//! `wav.rs` ile aynı `f64` mono örnek formatını üretir, böylece aynı `session::decode_wav_*`
//! fonksiyonları hem dosyadan hem canlı akıştan beslenebilir.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
