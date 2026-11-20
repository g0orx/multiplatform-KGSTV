# Atölye m-KGSTV — Cross-Platform Rewrite

**Goal:** to produce a rewrite of the Windows-only **KG-STV** (digital SSTV) software written by
JJ0OBZ (K.G., Niigata-Myoko, Japan) that runs on Windows, Linux, macOS and (if possible)
iOS/Android, and is compatible with original KG-STV stations on the air.

## Status

The protocol has been **verified** by bit-level reverse engineering against a translation of the
original `readme.txt` + real off-air audio recordings (4 different stations: TA4SO, TA2VB, DL4CQ
+ controlled OH2UDS test recordings). Details: [`docs/protocol_spec.md`](docs/protocol_spec.md).

| Layer | Status |
|---|---|
| Framing (preamble/sync/frame-walk) | ✅ Verified against a real recording |
| Convolutional code (K=7, 171/133 octal) | ✅ Verified, implemented in Rust |
| Whitening (127-bit M-sequence) | ✅ Verified, implemented in Rust |
| CRC (CRC-16/AUG-CCITT) | ✅ Verified, implemented in Rust |
| MSK demodulation | ✅ Production quality (Python), verified with real stations |
| JPEG Huffman/entropy tables | ✅ Standard tables verified bit-compatible |
| JPEG quantization scale (`sc`→scale) | ✅ Solved — `sc` is directly the IJG quality (1-100) |
| JPEG intra-block DC coding | ✅ Critical bug found and fixed (the 6 blocks are not chained to each other) |
| **Rust JPEG codec** (`kgstv-core::jpeg`) | ✅ Encode+decode, verified with real recording data |
| **Rust audio I/O + MSK modulator/demodulator** (`kgstv-modem`) | ✅ Verified end to end: 300/300 tiles decoded with zero bit errors from a real OH2UDS MSK recording |
| **4L-FSK MLSD/Viterbi demodulator** (`kgstv-modem::fsk4`) | ✅ Solved — 6-state CPM phase-trellis Viterbi + fine timing-shift scan; 96-99.7% tile success rate on real recordings (a big leap from the earlier ~80% symbol-accuracy bottleneck) |
| **Desktop GUI** (`kgstv-gui`, egui/eframe) | ✅ Single code base for Win/Linux/macOS — **RX**: open WAV, progressive tile drawing, frame log, live (microphone) decode, eye diagram, MSK+4L-FSK automatic; **TX**: send image/text (MSK or 4L-FSK), save as WAV or play through the speaker |
| **TCI radio control + audio** (`kgstv-modem::tci`) | ✅ Connect to a TCI server (ExpertSDR, piHPSDR, deskHPSDR ...), show/tune frequency and mode, receive audio from the radio for live decode, transmit through the radio (keys PTT, TX audio paced by TX_CHRONO); verified against a mock TCI server end to end |
| iOS/Android UI | ⏳ Not started yet |

## Directory Structure

- `docs/` — research and protocol documentation
  - `research.md` — web research findings
  - `protocol_spec.md` — **authoritative source**: full protocol specification + verification records
  - `HISTORY.md`, `OVERVIEW_EN.md`, `kgstv-protocol-reference.html` — English history/technical overview
- `research/` — reverse-engineering workspace
  - `binaries/` — the original `kgstv.zip` (KGSTV.exe, readmes, kgstv.ini)
  - `sampleaudio/` — synthetic test recordings (MSK/4LFSK, Norm/Convolution, different JPEG scales)
  - `decoder/` — Python prototype decoder:
    - `kgstv_decode.py`, `tone_demod.py` — low-level demod/framing helpers
    - `decode_session.py` — **main tool**: decodes a WAV recording from start to finish and produces a PNG
    - `jpeg_block.py` — JPEG 16×16 block decode reference implementation (Python)
    - `kgstv_filters.py` — real FIR filters extracted from `kgstv.exe` by disassembly
- `samples/` — real off-air recordings (TA4SO, TA2VB, DL4CQ) and controlled test recordings
  (OH2UDS), together with reference images
  - `samples/decoded/` — decoded output PNGs
- `src/kgstv-core/` — Rust core library (cargo workspace member), platform-independent
  protocol implementation: framing, convolutional code, whitening, CRC, header, **JPEG codec**
- `src/kgstv-modem/` — Rust audio layer (cargo workspace member, built on top of
  `kgstv-core`): WAV I/O (`wav`), band-pass pre-filter (`filter`), true continuous-phase MSK
  modulation/demodulation + tone-correlation bit detection (`msk`), **4L-FSK MLSD/Viterbi
  modulation/demodulation** (`fsk4` — 6-state CPM phase trellis), sync-word search
  (`sync`), top-level TX/RX session API (`session`; header/sync always MSK + data chunk
  MSK/4L-FSK automatically according to `hdr.m`, including encoding with a progress callback),
  continuous sample capture from the default microphone/soundcard + playback through the speaker
  (`live`, via `cpal`), and a TCI client (`tci`) for radio control plus RX/TX audio over
  WebSocket.
  `examples/decode_wav.rs`: a CLI tool that decodes a WAV file end to end and produces a PPM image.
- `src/kgstv-gui/` — desktop GUI (cargo workspace member, egui/eframe), RX and TX modes:
  **RX** — open a WAV file (native file picker, `rfd`) or start live (microphone) decoding,
  decode in the background with `kgstv-modem` (MSK/4L-FSK detected automatically), draw tiles
  live as they arrive, show the frame log; **TX** — scales an image file (PNG/JPEG/GIF/BMP via the
  `image` crate) to 320×240 and encodes it into 300 JPEG tiles + an optional callsign/text
  (MSK or 4L-FSK modulation selectable), saves it as a WAV or plays it directly through the
  speaker. In both modes an eye diagram is shown for assessing signal quality/timing.
  A **📻 Radio (TCI)** section in both modes connects to a TCI server (default port 40001),
  tunes the radio, and lets live decode and transmit use the radio's audio instead of a sound card.
  Single code base for Windows/Linux/macOS.

## Development

```sh
# Test the Rust core (+ modem layer)
cargo test --workspace

# Rust CLI: decode a real/synthetic MSK WAV recording end to end (produces a PPM image)
cargo run -p kgstv-modem --release --example decode_wav -- samples/<recording>.wav output.ppm

# Run the desktop GUI
cargo run -p kgstv-gui --release

# Python prototype decoder: decode a WAV recording to PNG
cd research && source venv/bin/activate
python3 decoder/decode_session.py ../samples/<recording>.wav ../samples/decoded/<output>.png
```

## Next Steps

See the project task list and the relevant sections of `docs/protocol_spec.md`:
1. Port the sequential frame walk in `kgstv-modem::session` to the independent-candidate scanning
   approach of `decode_session.py` for noisy real off-air recordings (on long/noisy recordings
   the sequential walk stops on loss of sync)
2. For 4L-FSK's few remaining tile errors (287-299/300, probably real channel noise), a
   wider/finer phase+timing scan or a soft-decision CRC/CONV combination could be tried
   — a partial improvement, not a blocker
3. iOS/Android UI (egui already supports Android/iOS builds — the desktop code base should be
   portable; platform-specific audio I/O integration is needed separately)
