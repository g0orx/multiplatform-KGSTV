//! WAV dosya okuma/yazma (hound üzerinden), platformdan bağımsız ses I/O.

use std::path::Path;

pub struct AudioBuffer {
    pub sample_rate: f64,
    pub samples: Vec<f64>,
}

/// Bir WAV dosyasını okur, mono'ya indirger (çok kanallıysa ortalama alır), örnekleri
/// -1.0..1.0 aralığında normalize eder. Örnekleme hızı dosyadan olduğu gibi okunur —
/// sabit bir değer VARSAYILMAZ (bkz. docs/protocol_spec.md, gerçek kayıtlar 8000Hz-44100Hz
/// arası değişik hızlarda olabiliyor).
pub fn load_mono<P: AsRef<Path>>(path: P) -> Result<AudioBuffer, hound::Error> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    let raw: Vec<f64> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f64;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f64 / max))
                .collect::<Result<Vec<_>, _>>()?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map(|v| v as f64))
            .collect::<Result<Vec<_>, _>>()?,
    };

    let mono: Vec<f64> = if channels > 1 {
        raw.chunks(channels)
            .map(|c| c.iter().sum::<f64>() / channels as f64)
            .collect()
    } else {
        raw
    };

    Ok(AudioBuffer { sample_rate: spec.sample_rate as f64, samples: mono })
}

/// Mono örnekleri (-1.0..1.0 aralığında varsayılır, kırpılır) 16-bit PCM WAV olarak yazar.
pub fn save_mono<P: AsRef<Path>>(path: P, sample_rate: u32, samples: &[f64]) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f64).round() as i16;
        writer.write_sample(v)?;
    }
    writer.finalize()?;
    Ok(())
}
