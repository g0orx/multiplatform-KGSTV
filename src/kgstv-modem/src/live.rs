//! Live audio capture (RX) and playback (TX): via `cpal` with the default input/output devices
//! — Windows (WASAPI), Linux (ALSA), macOS (CoreAudio). The input side produces the same
//! `f64` mono sample format as `wav.rs`, so the same `session::decode_wav_*` functions can be fed
//! from both a file and a live stream. The output side plays a modulated signal produced by
//! `session::encode_session_audio` directly to the speaker/sound card.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Sample buffer that keeps growing but is trimmed from the front once it exceeds a certain
/// duration (ring-buffer-like). Once `max_samples` is exceeded the oldest samples are dropped — so memory does not grow unbounded.
struct RingBuffer {
    data: VecDeque<f64>,
    max_samples: usize,
    /// The ABSOLUTE sample index of `data[0]` (counted from the start of the session, as if the buffer
    /// had never been trimmed) — i.e. the total number of samples dropped from the front so far. Once
    /// the buffer fills and old samples start being dropped, this is needed so that `kgstv-gui` can
    /// notice and correct the fact that a cached anchor (a sample position computed relative to a
    /// previous `snapshot()`) has SHIFTED relative to a NEW `snapshot()` (see the `kgstv-gui`
    /// `maybe_kick_live_decode` documentation — user feedback: "it works fine the first
    /// time but then ... it tried to receive all the frames again").
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

/// An audio device as offered to the user: `id` is cpal's unique, stable device identifier
/// (e.g. `alsa:plughw:CARD=E900Plus,DEV=0`) used to find the device again; `label` is a
/// human-readable name for a dropdown.
///
/// Devices must be identified by `id`, NOT by display name: names are not unique (one ALSA card
/// typically appears as several entries — `hw`, `plughw`, `sysdefault`, `front`, `dsnoop` — all
/// with the same name), and the default device's name ("Default Audio Device") does not even
/// match the name under which the same device appears in the enumerated list, so a by-name
/// lookup of the default device always failed ("Input device not found: Default Audio Device").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub label: String,
}

fn device_info(d: &cpal::Device) -> Option<AudioDeviceInfo> {
    // (cpal 0.18: the device name comes via `Display`/`to_string()`, not `Device::name()`.)
    let id = d.id().ok()?.to_string();
    // The host prefix ("alsa:", "wasapi:" ...) adds nothing for the user — show only the rest.
    let short_id = id.split_once(':').map(|(_, rest)| rest).unwrap_or(&id);
    Some(AudioDeviceInfo { label: format!("{d} [{short_id}]"), id })
}

/// Lists the system's audio input devices (for populating a dropdown).
pub fn list_input_devices() -> Vec<AudioDeviceInfo> {
    match cpal::default_host().input_devices() {
        Ok(it) => it.filter_map(|d| device_info(&d)).collect(),
        Err(_) => Vec::new(),
    }
}

/// Lists the system's audio output devices (for populating a dropdown).
pub fn list_output_devices() -> Vec<AudioDeviceInfo> {
    match cpal::default_host().output_devices() {
        Ok(it) => it.filter_map(|d| device_info(&d)).collect(),
        Err(_) => Vec::new(),
    }
}

/// Name of the default input device.
pub fn default_input_device_name() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| d.to_string())
}

/// Name of the default output device.
pub fn default_output_device_name() -> Option<String> {
    cpal::default_host().default_output_device().map(|d| d.to_string())
}

/// `id`: an `AudioDeviceInfo::id`, or `None` for the system default.
fn find_input_device(id: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match id {
        Some(id) => host
            .input_devices()
            .map_err(|e| format!("Could not list input devices: {e}"))?
            .find(|d| d.id().map(|i| i.to_string() == id).unwrap_or(false))
            .ok_or_else(|| format!("Input device not found: {id}")),
        None => host.default_input_device().ok_or_else(|| "Default audio input device not found".to_string()),
    }
}

/// `id`: an `AudioDeviceInfo::id`, or `None` for the system default.
fn find_output_device(id: Option<&str>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    match id {
        Some(id) => host
            .output_devices()
            .map_err(|e| format!("Could not list output devices: {e}"))?
            .find(|d| d.id().map(|i| i.to_string() == id).unwrap_or(false))
            .ok_or_else(|| format!("Output device not found: {id}")),
        None => host.default_output_device().ok_or_else(|| "Default audio output device not found".to_string()),
    }
}

/// An active live capture session. The stream stops when it is `Drop`ped.
pub struct LiveCapture {
    pub sample_rate: f64,
    buffer: Arc<Mutex<RingBuffer>>,
    gain: Arc<Mutex<f32>>,
    /// Peak amplitude of the most recently captured chunk (from 0.0 upward, typically ~0-1.5) —
    /// the GUI's VU meter reads this periodically.
    level: Arc<Mutex<f32>>,
    _stream: cpal::Stream,
}

impl LiveCapture {
    /// Starts capturing from the given (or, if `None`, the default) input device.
    /// `max_seconds`: maximum duration to keep in the buffer (older samples are dropped automatically).
    /// `initial_gain`: multiplier applied to the samples (1.0 = no change) — can be changed live
    /// later via `set_gain()`.
    pub fn start(max_seconds: f64, device_name: Option<&str>, initial_gain: f32) -> Result<Self, String> {
        let device = find_input_device(device_name)?;
        let config = device
            .default_input_config()
            .map_err(|e| format!("Could not get input configuration: {e}"))?;

        let sample_rate = config.sample_rate() as f64;
        let channels = config.channels() as usize;
        let max_samples = (max_seconds * sample_rate).round() as usize;
        let buffer = Arc::new(Mutex::new(RingBuffer { data: VecDeque::new(), max_samples, front_absolute_index: 0 }));
        let buffer_cb = buffer.clone();
        let gain = Arc::new(Mutex::new(initial_gain));
        let gain_cb = gain.clone();
        let level = Arc::new(Mutex::new(0.0f32));
        let level_cb = level.clone();

        let err_fn = |e| eprintln!("[kgstv-modem::live] audio stream error: {e}");
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
                .map_err(|e| format!("Could not build input stream: {e}"))?,
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
                .map_err(|e| format!("Could not build input stream: {e}"))?,
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
                .map_err(|e| format!("Could not build input stream: {e}"))?,
            fmt => return Err(format!("Unsupported sample format: {fmt:?}")),
        };

        stream.play().map_err(|e| format!("Could not start audio stream: {e}"))?;

        Ok(LiveCapture { sample_rate, buffer, gain, level, _stream: stream })
    }

    /// Returns a copy of the samples accumulated so far (for decoding).
    pub fn snapshot(&self) -> Vec<f64> {
        let buf = self.buffer.lock().unwrap();
        buf.data.iter().copied().collect()
    }

    /// Number of samples accumulated so far.
    pub fn len(&self) -> usize {
        self.buffer.lock().unwrap().data.len()
    }

    /// The ABSOLUTE index (counted from the start of the session) of the FIRST sample of the array
    /// `snapshot()` will return — 0 if the buffer has not been trimmed yet, otherwise the number of
    /// samples dropped so far. Used to compute how far a previously cached anchor (computed relative
    /// to an old `snapshot()`) has SHIFTED relative to a new `snapshot()`
    /// (see `kgstv-gui` `maybe_kick_live_decode`).
    pub fn front_absolute_index(&self) -> u64 {
        self.buffer.lock().unwrap().front_absolute_index
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Updates the input gain (multiplier; 1.0 = no change) LIVE — no need to restart
    /// the stream.
    pub fn set_gain(&self, gain: f32) {
        *self.gain.lock().unwrap() = gain.max(0.0);
    }

    pub fn gain(&self) -> f32 {
        *self.gain.lock().unwrap()
    }

    /// Peak amplitude of the most recently captured chunk — for the VU meter (the GUI reads it periodically).
    pub fn level(&self) -> f32 {
        *self.level.lock().unwrap()
    }
}

/// Simulator that feeds the samples of an already-loaded WAV into a shared buffer at REAL-TIME speed
/// (chunk by chunk, with `sleep`, over a total time equal to the real playback duration).
/// Same interface shape as `LiveCapture` (`sample_rate`, `snapshot()`, `len()`) — so the caller
/// (`kgstv-gui`) can handle real microphone listening and "playing a WAV file as if live" with
/// the same progressive decode/eye-diagram code. Unlike an instant decode (which processes the whole
/// file at once), it lets the eye diagram see ONLY the audio "received so far" and makes the
/// tiles appear on screen at real KG-STV speed.
pub struct PlaybackSim {
    pub sample_rate: f64,
    total_len: usize,
    buffer: Arc<Mutex<Vec<f64>>>,
    done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl PlaybackSim {
    /// Starts feeding `samples` at `sample_rate` speed in ~100ms chunks
    /// (on a background thread) and returns immediately.
    pub fn start(samples: Vec<f64>, sample_rate: f64) -> Self {
        let total_len = samples.len();
        let buffer = Arc::new(Mutex::new(Vec::with_capacity(total_len)));
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let buf_cb = buffer.clone();
        let done_cb = done.clone();
        let stop_cb = stop.clone();
        std::thread::spawn(move || {
            let chunk_len = ((sample_rate * 0.1) as usize).max(1); // ~100ms chunks
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

    /// A copy of the samples that have "arrived" so far.
    pub fn snapshot(&self) -> Vec<f64> {
        self.buffer.lock().unwrap().clone()
    }

    /// Number of samples received so far.
    pub fn len(&self) -> usize {
        self.buffer.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Has the whole file been fed (has playback finished)?
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// Total number of samples (the whole file) — for showing progress.
    pub fn total_len(&self) -> usize {
        self.total_len
    }
}

impl Drop for PlaybackSim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Plays the given mono samples (-1.0..1.0, `sample_rate` Hz) on the given (or, if `None`, the
/// default) audio output device, blocking (the calling thread) until playback finishes.
/// `gain`: multiplier applied to the samples before playback (1.0 = no change). For feeding the TX
/// signal produced by `session::encode_session_audio` directly to the speaker/sound card
/// — the caller should call this from a separate thread so the UI does not freeze.
///
/// `stop`: if given, it is checked periodically during playback — when it becomes `true`
/// playback is cut off IMMEDIATELY (the stream is dropped) and the function returns early. Since a
/// transmission (especially a full image) can take minutes, this lets the user cancel the "Play"
/// step (user request: "a button that lets me stop the play step while sending").
pub fn play_blocking(
    samples: &[f64],
    sample_rate: f64,
    device_name: Option<&str>,
    gain: f32,
    stop: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
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
        .map_err(|e| format!("Could not get output configurations: {e}"))?
        .collect();

    // Many real output devices (e.g. built-in speakers) support ONLY their own fixed sample rate
    // (usually 48000 Hz), not 44100 Hz. If the requested rate is not supported, fall back to the
    // device's default configuration and resample the audio to that rate (simple linear
    // interpolation — since the TX signal's bandwidth is low (500-2500Hz) this can be considered
    // practically lossless).
    let exact = ranges.iter().find(|r| r.min_sample_rate() <= want_sr && want_sr <= r.max_sample_rate());
    let (supported, playback_samples): (cpal::SupportedStreamConfig, Vec<f64>) = match exact {
        Some(range) => (range.clone().with_sample_rate(want_sr), samples.to_vec()),
        None => {
            let default_cfg = device
                .default_output_config()
                .map_err(|e| format!("Could not get default output configuration: {e}"))?;
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
    let err_fn = |e| eprintln!("[kgstv-modem::live] output stream error: {e}");

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
                .map_err(|e| format!("Could not build output stream: {e}"))?
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
                .map_err(|e| format!("Could not build output stream: {e}"))?
        }
        fmt => return Err(format!("Unsupported output sample format: {fmt:?}")),
    };

    stream.play().map_err(|e| format!("Could not start output stream: {e}"))?;
    // Since `total`/`samples` may have been resampled, the duration must be computed with
    // `effective_sample_rate` (the rate the device actually plays at) — NOT the original `sample_rate`.
    let duration = Duration::from_secs_f64(total as f64 / effective_sample_rate + 0.3);
    // Instead of one long `sleep`, sleep in short intervals and check the `stop` flag —
    // if the user cancels, the stream stops IMMEDIATELY (when `stream` is dropped by returning early
    // from this function), with no need to wait for minutes.
    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    let mut elapsed = Duration::ZERO;
    while elapsed < duration {
        if let Some(s) = &stop {
            if s.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
        let step = POLL_INTERVAL.min(duration - elapsed);
        std::thread::sleep(step);
        elapsed += step;
    }
    Ok(())
}

/// Resampling by simple linear interpolation (`from_sr` Hz -> `to_sr` Hz). Not a high-quality
/// resampler, but sufficient for KG-STV's narrow band (500-2500Hz), purely for playback through
/// the speaker — WAV recording/decoding always stays at the original (non-resampled) rate.
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

    /// `front_absolute_index` must increase correctly once the buffer exceeds capacity and old
    /// samples start being dropped — `kgstv-gui` relies on this to correctly re-localize a cached
    /// anchor as the buffer is trimmed (see the `LiveCapture::front_absolute_index`
    /// documentation).
    #[test]
    fn ring_buffer_tracks_front_absolute_index_as_it_evicts() {
        let mut buf = RingBuffer { data: VecDeque::new(), max_samples: 5, front_absolute_index: 0 };

        buf.push_slice(&[1.0, 2.0, 3.0]);
        assert_eq!(buf.front_absolute_index, 0); // no trimming yet (3 < capacity 5)
        assert_eq!(buf.data.iter().copied().collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);

        buf.push_slice(&[4.0, 5.0, 6.0, 7.0]); // total 7 > capacity 5 -> 2 are dropped
        assert_eq!(buf.front_absolute_index, 2);
        assert_eq!(buf.data.iter().copied().collect::<Vec<_>>(), vec![3.0, 4.0, 5.0, 6.0, 7.0]);

        buf.push_slice(&[8.0]);
        assert_eq!(buf.front_absolute_index, 3);
        assert_eq!(buf.data.iter().copied().collect::<Vec<_>>(), vec![4.0, 5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn playback_sim_grows_over_time_and_finishes() {
        let sr = 4410.0; // low rate: so the test finishes quickly (~100ms chunk -> 441 samples)
        let samples: Vec<f64> = (0..(sr as usize) / 2).map(|i| i as f64).collect(); // 0.5 s of data
        let sim = PlaybackSim::start(samples.clone(), sr);
        assert_eq!(sim.total_len(), samples.len());
        assert!(sim.is_empty());
        assert!(!sim.is_done());

        std::thread::sleep(Duration::from_millis(150));
        let mid = sim.len();
        assert!(mid > 0 && mid < samples.len(), "mid={mid} (should be between 0 and {})", samples.len());

        std::thread::sleep(Duration::from_millis(600));
        assert!(sim.is_done());
        assert_eq!(sim.snapshot(), samples);
    }

    #[test]
    fn resample_linear_preserves_low_frequency_tone() {
        // A 1500 Hz tone (the centre of the KG-STV band) generated at 44100 Hz, when resampled to
        // 48000 Hz, should still be around ~1500 Hz with roughly the same amplitude (proof that simple
        // linear interpolation is sufficient for this narrow band — see `play_blocking`).
        let from_sr = 44100.0;
        let to_sr = 48000.0;
        let freq = 1500.0;
        let n = 4410; // 0.1 s
        let input: Vec<f64> =
            (0..n).map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / from_sr).sin()).collect();
        let output = resample_linear(&input, from_sr, to_sr);

        // Duration must be preserved (± a few samples).
        let expected_len = (n as f64 * to_sr / from_sr).round() as usize;
        assert!((output.len() as i64 - expected_len as i64).abs() <= 2, "len={}", output.len());

        // Is there still oscillation around ~1500Hz at the output: check with a simple zero-crossing count.
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
