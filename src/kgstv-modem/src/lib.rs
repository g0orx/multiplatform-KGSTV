//! kgstv-modem: KG-STV için ses I/O + MSK modülatör/demodülatör.
//!
//! `kgstv-core`'un doğrulanmış bit-seviyesi protokol mantığı (framing, konvolüsyonel kod,
//! whitening, CRC, JPEG tile codec) üzerine ses katmanını ekler: WAV okuma/yazma, bant-geçiren
//! ön-filtre, gerçek sürekli-faz MSK sentezi/demodülasyonu, senkron kelimesi arama ve bunların
//! hepsini bir araya getiren üst seviye oturum (session) API'si.
//!
//! 4L-FSK ses katmanı henüz burada YOK — ton-korelasyonu demodülasyonu ~%80-83 sembol
//! doğruluğunda kalıyor (bkz. docs/protocol_spec.md), pratik decode için yetersiz; uygun bir
//! MLSD/Viterbi tabanlı CPFSK demodülatörü ayrı bir iş olarak planlanıyor.

pub mod wav;
pub mod filter;
pub mod msk;
pub mod fsk4;
pub mod sync;
pub mod session;
pub mod live;

pub use wav::{load_mono, save_mono, AudioBuffer};
pub use filter::bandpass_filtfilt;
pub use msk::{modulate_msk, ToneClock, BAUD, MSK_FREQS};
pub use fsk4::{bits_to_levels, levels_to_bits, modulate_fsk4, Fsk4Detector};
pub use sync::{find_sync, sync_err, SYNC_LEN};
pub use session::{
    decode_wav_sequential, decode_wav_sequential_cb, encode_session_audio, encode_session_audio_cb,
    FrameResult,
};
pub use live::{play_blocking, LiveCapture, PlaybackSim};
