# KG-STV: History, the Reverse-Engineering Effort, and a Technical Introduction

## Part 1 — Origin and Background

KG-STV is a digital slow-scan television (SSTV) mode and software application for amateur
radio, created by an operator known by the callsign **JJ0OBZ**, who signs the software as
**"K.G."**, based in Niigata Prefecture, Myōkō City, Japan. Unlike classic analogue SSTV modes
(Martin, Scottie, Robot36, etc.), which encode a picture as a continuously varying audio tone
that traces brightness line by line, KG-STV takes a fully digital approach: it compresses an
image with JPEG-style encoding, splits it into small 16×16-pixel blocks, and transmits each
block as an independently error-checked digital packet over a normal SSB or FM voice channel,
using nothing more than a computer's sound card and a serial (RS-232) PTT connection to the
radio — the same kind of interface hobbyists already used for other ham radio digital modes.

The author built KG-STV purely out of personal interest in digital image transmission, and
said so candidly in the software's own documentation: it was an experiment, released so other
amateurs could try it, report back, and help refine it — not a polished commercial product.
That spirit shows in the design: a Bad Segment Report (BSR) mechanism lets a receiving station
ask the sender to retransmit just the blocks that were lost or corrupted, so a full picture
can still resolve even through a noisy or fading HF path, and even a transmission that only
got partially received still shows most of the image rather than nothing at all.

KG-STV found a real, if niche, following in the SSTV and digital-modes community. Off-air
logs and blog posts from the late 2010s show activity on the classic HF SSTV calling
frequencies (3733 kHz, 7173 kHz, 14233 kHz) and — notably — over the QO-100 geostationary
amateur satellite transponder (10.489625 GHz), where a stable, near-noise-free channel let
operators push KG-STV's higher-fidelity 4-level FSK mode and enjoy sharper, more reliable
image transfers than HF weather would normally allow. Stations logged exchanging KG-STV
images include callsigns such as DD4YR, DL9DAC, IZ5ILX, G3YPQ, SV8CS, and IZ1ERR, among
others — a small international group of enthusiasts who kept the mode alive by using it.

Here is where KG-STV became difficult for the wider ham radio world to adopt: **the software
and its documentation were written by and for a Japanese-speaking audience, and the program
itself only ever ran on Windows.** The installation archive ships three successive readme
files (covering versions 1.0.1 build 002 through 004) written entirely in Japanese, encoded in
Shift-JIS — a character encoding that, opened naively on a non-Japanese system, renders as
unreadable garbage rather than legible text. Critically, it is *this* Japanese-only readme
that contains the software's actual technical transmission specification: exact modulation
frequencies, the bit-level frame structure, error-correction scheme, and synchronization
sequences — information that, as far as could be determined, was never independently
republished in English in a complete, technically usable form. A shortcut link in the archive
(`KG-STV English Tech info.URL`) hints that the author or the community once tried to bridge
that language gap, but no authoritative English protocol reference equivalent to the Japanese
original was found circulating publicly. Compounding this, KG-STV was written for the classic
Windows desktop environment of its era (the shipped binary dates to 2010, built with a
Borland/Embarcadero Delphi or C++Builder toolchain) and never had a Linux, macOS, or mobile
counterpart.

## Part 2 — Our Reverse-Engineering Journey

Our goal from the outset was narrow and concrete: make KG-STV usable on Windows, Linux,
macOS, and, if feasible, iOS and Android — while staying compatible with the real,
on-air KG-STV community, not just building a look-alike. That requirement ruled out guessing;
every detail of the wire protocol had to be either found in an authoritative source or proven
against a real signal.

We started the way any such investigation should: searching the open web for whatever the
amateur radio community had already published about KG-STV. Scattered but useful pieces
turned up — the Signal Identification Wiki's KG-STV entry, a technical write-up on
M5AML/wells10000's personal site, a plain-language explainer on Johnson Francis's
"Techworld" blog, a hands-on reception guide on sgcderek's blog, an Italian SWL blogger's
QO-100 logs, and old mailing-list threads on `digitalradio@yahoogroups.com` (mirrored on
mail-archive.com) and `groups.io`, including one explicitly asking whether an open-source
KG-STV implementation existed. None of these, on their own, gave a complete or verifiable
protocol specification — they confirmed the broad strokes (MSK/4-level FSK, 1200 baud,
16×16 JPEG blocks, a BSR retry scheme) but not the bit-exact framing we needed to actually
interoperate.

The real breakthrough came from going to the primary source: the original KG-STV
distribution archive itself, obtained from a ham radio software mirror
(qsl.net/k/kb1uif). Inside it, alongside the Windows executable and a folder of previously
received sample images, sat the three Japanese-language readme files. Translating those
properly (correcting the Shift-JIS encoding first, since it displays as corrupted text
otherwise) revealed that the author had, in fact, documented the entire wire protocol in
detail — modulation frequencies, the 54-bit header layout down to the individual bit, the
command table, the synchronization word, even the whitening sequence — it had simply never
been available in English.

Even a translated specification is still just a claim on paper, so we did not treat it as
ground truth until we had tested it. Working from real off-air audio recordings of actual
KG-STV transmissions (in MSK and 4-level FSK, with and without convolutional coding, at
different JPEG quality settings), we built a small signal-processing prototype from scratch
and demodulated those recordings bit by bit: locating the synchronization pattern, decoding
the convolutional code with a Viterbi decoder, undoing the whitening sequence, and checking
the result against a CRC. When a real recording of a test message decoded — cleanly, byte
for byte — into the plain sentence *"TEST3>The Quick Brown Fox Jumps Over The Lazy Dog
0123456789"*, and separately into 22 real, CRC-verified 16×16 image blocks from an actual
picture transmission, that was the confirmation that the translated specification was not
just plausible but bit-exact. Only at that point did we begin porting the validated protocol
into the portable, cross-platform Rust implementation that this project is now built on.

Two details are still open at the time of writing — the exact quantization-scale mapping used
for JPEG blocks, and reliable calibration of the 4-level FSK demodulator — and are tracked
honestly as such rather than papered over. Everything else in the sections below has been
independently confirmed against real signals, not just read out of the documentation.

## Part 3 — Technical Introduction to KG-STV

### What kind of mode is it?

KG-STV belongs to the SSTV family in the sense that it sends a picture over a normal voice
radio channel using an audio tone, but it is digital rather than analogue throughout. Where
classic SSTV modes convert brightness directly into a varying tone frequency line by line,
KG-STV JPEG-compresses the image first, cuts it into 16×16-pixel tiles, and sends each tile
as a small, independently checksummed digital packet. A full image is normalized to
320×240 pixels (cropping or padding as needed), giving a 20×15 grid of tiles. Alongside
images, the same digital channel carries short text messages and callsign announcements.

### The retry mechanism (BSR)

Because each tile travels as its own packet with its own error check, a receiving station
can tell exactly which tiles arrived cleanly and which did not. If some are missing or
corrupted, the receiver can send a **Bad Segment Report (BSR)** back to the transmitting
station, listing the coordinates of the bad tiles; the transmitter then resends just those
tiles. This means a picture can still resolve fully even over a fading or interference-prone
HF path, and a station that only received part of a transmission still sees a mostly-complete
image rather than nothing.

### Modulation

KG-STV offers a choice of two modulation schemes, both running at a constant **1200 baud**
within roughly a 500–2500 Hz audio bandwidth, transmitted as ordinary SSB or FM voice audio
(F1D / F2D emission):

- **MSK (Minimum Shift Keying)** — a continuous-phase binary FSK variant, using **1200 Hz**
  for a `0` bit and **1800 Hz** for a `1` bit. The continuous-phase property keeps the
  transmitted envelope constant, which lets the radio's power amplifier run at full efficiency
  and improves range compared with amplitude-sensitive schemes like PSK or OFDM.
- **4-level FSK (4L-FSK)** — the same 1200-baud symbol rate, but each symbol carries two bits
  by choosing among four tones: `00`→1200 Hz, `01`→1400 Hz, `10`→1600 Hz, `11`→1800 Hz. This
  doubles the effective data rate compared with MSK, at the cost of being more sensitive to
  noise and a degraded channel — a trade-off KG-STV leaves to the operator, who typically
  favours 4L-FSK on a clean FM or satellite link and MSK on a fading HF/SSB path.

Optionally, either modulation can be paired with a **rate-1/2, constraint-length-7
convolutional code** (the same "NASA standard K=7" code family used in deep-space
telemetry), roughly doubling transmission time in exchange for strong resilience to noise.
The synchronization/header portion of every transmission is *always* sent with MSK plus this
convolutional coding, regardless of which mode the operator picked for the image data itself,
so that every receiver can reliably lock on and read the header before it even knows which
mode the payload will use.

### Framing, at a glance

Every transmission begins with a 256-bit alternating `01` preamble (for the receiver's clock
recovery), followed by a fixed 63-bit synchronization word. What follows is a repeating unit
of *(synchronization word) + (54-bit header) + (payload chunk)* — one such unit per image
tile, or a single one for a short text message. The header carries a command code (text,
image, callsign, BSR request/response, end, or abort), the coding and modulation mode in use,
the tile's grid coordinates, its JPEG quality/scale setting, the payload length, and its own
CRC. The payload chunk carries the actual data — JPEG-compressed pixel data for an image tile,
or plain text — followed by its own 16-bit CRC. A 127-bit whitening sequence is XORed over
each chunk (restarting at the start of every chunk) to avoid long runs of identical tones
that would otherwise make clock recovery difficult.

### Image compression

Each 16×16 tile is compressed as a small, self-contained JPEG-style block: four 8×8 luma
(Y) blocks plus one 8×8 Cb and one 8×8 Cr block (i.e. 4:2:0-equivalent colour subsampling,
described in the original documentation as "4:1:1"), each run through the standard JPEG
discrete cosine transform, quantization, and Huffman entropy coding — but packed as a bare
bitstream, without the marker bytes, byte alignment, or headers a normal standalone JPEG
file would carry, since a shared, implicit table set makes that overhead unnecessary for such
small tiles.
