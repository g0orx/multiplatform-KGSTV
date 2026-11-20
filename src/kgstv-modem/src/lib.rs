//! kgstv-modem: audio I/O + MSK modulator/demodulator for KG-STV.
//!
//! Adds the audio layer on top of `kgstv-core`'s verified bit-level protocol logic (framing,
//! convolutional code, whitening, CRC, JPEG tile codec): WAV reading/writing, a band-pass
//! pre-filter, true continuous-phase MSK synthesis/demodulation, sync-word search, and a
//! top-level session API that ties them all together.
//!
//! The 4L-FSK audio layer is NOT here yet — tone-correlation demodulation stays at ~80-83% symbol
//! accuracy (see docs/protocol_spec.md), which is insufficient for practical decoding; a proper
//! MLSD/Viterbi-based CPFSK demodulator is planned as separate work.

pub mod wav;
pub mod filter;
pub mod msk;
pub mod fsk4;
pub mod discriminator;
pub mod sync;
pub mod session;
pub mod live;
pub mod tci;

pub use wav::{load_mono, save_mono, AudioBuffer};
pub use filter::{bandpass_filtfilt, lowpass_filtfilt};
pub use msk::{modulate_msk, ToneClock, BAUD, MSK_FREQS};
pub use fsk4::{bits_to_levels, levels_to_bits, modulate_fsk4, Fsk4Detector};
pub use discriminator::instantaneous_freq;
pub use sync::{find_sync, sync_err, SYNC_LEN};
pub use session::{
    decode_wav_sequential, decode_wav_sequential_cb, decode_wav_sequential_cb_from_anchor,
    encode_session_audio, encode_session_audio_cb, FrameResult,
};
pub use live::{
    AudioDeviceInfo, default_input_device_name, default_output_device_name, list_input_devices, list_output_devices,
    play_blocking, LiveCapture, PlaybackSim,
};
pub use tci::{TciClient, TciState, TciStatus, TCI_AUDIO_SAMPLE_RATE, TCI_DEFAULT_PORT};
