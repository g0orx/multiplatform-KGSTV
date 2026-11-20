//! WAV file reading/writing (via hound), platform-independent audio I/O.

use std::path::Path;

pub struct AudioBuffer {
    pub sample_rate: f64,
    pub samples: Vec<f64>,
}

/// Reads a WAV file, downmixes it to mono (averaging if multi-channel), and normalizes the
/// samples to the -1.0..1.0 range. The sample rate is read from the file as-is —
/// NO fixed value is ASSUMED (see docs/protocol_spec.md; real recordings can come at various
/// rates between 8000Hz and 44100Hz).
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

/// Writes mono samples (assumed to be in -1.0..1.0, clipped) as a 16-bit PCM WAV.
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
