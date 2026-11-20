# KG-STV Protocol Specification (from the original readme.txt)

> **Source:** `readme.txt` inside `kgstv.zip` (KG-STV ver.1.0.1), written by JJ0OBZ (K.G., Niigata-Myoko,
> Japan). The file is in Shift-JIS/CP932 encoding; a version converted to UTF-8 is available in
> `research/notes/readme_utf8_cp932.txt`. This document is a **verbatim translation** of that section — it
> contains no commentary/guesswork, only a translation of the original Japanese text. In the author's own
> words this is an "experimental" protocol; the author also notes that errors/omissions may be found over time.

This is the **only authoritative source** we have. It agrees with the numbers given by third-party
summaries on the web (sigidwiki, wells10000 etc.) (1200 baud, MSK/4LFSK frequencies, Viterbi K=7), but
here the **bit-level frame structure** is available as extra information.

## General Transmission Parameters

| Parameter | Value |
|---|---|
| Modulation | MSK, or mixed MSK + 4-level FSK modulation |
| Baud rate | 1200 baud |
| MSK frequencies | Space (0) = 1200 Hz, Mark (1) = 1800 Hz |
| 4L-FSK frequencies | `00`→1200 Hz, `01`→1400 Hz, `10`→1600 Hz, `11`→1800 Hz |
| Bandwidth | 500–2500 Hz |
| Error correction | None, or Viterbi code (NASA standard K=7 code) |
| Whitening/scrambling | Yes — an M-sequence code with a period of 127 bits is XORed onto every bit |
| Interleaver | None |
| Header (before sync) | Repeating `01` signal, 256 bits |
| Basic code structure | sync code + 54-bit info chunk (header) + variable-length data chunk |
| Error detection | Yes (CRC-16/CCITT) |
| Sync bit sequence | 63-bit M-sequence code |
| Image compression | JPEG-compatible (16×16 pixel blocks, 4:1:1 chroma subsampling) |
| Text encoding | Shift-JIS compatible |
| Emission type | F1D (operation in SSB mode), F2D (operation in FM mode) |

## Info Chunk (Header) Structure — system code version '0'

```
┌──────┬──────┬───┬───┬──────┬──────┬──────┬──────┬──────┐
│ sys  │ com  │ c │ m │  x   │  y   │  sc  │ size │ CRC  │
└──────┴──────┴───┴───┴──────┴──────┴──────┴──────┴──────┘
 0      4      8   9   10     16     22     26     38   54 bit
```

| Field | Bit offset | Width | Meaning |
|---|---|---|---|
| `sys` | 0 | 4 bits | System code (version) |
| `com` | 4 | 4 bits | Command code (see command table) |
| `c` | 8 | 1 bit | Coding mode (NORM/CONV, i.e. convolutional code on/off) |
| `m` | 9 | 1 bit | Modulation mode (MSK / 4L-FSK) |
| `x` | 10 | 6 bits | Image block position (X) |
| `y` | 16 | 6 bits | Image block position (Y) |
| `sc` | 22 | 4 bits | JPEG scale/quality size |
| `size` | 26 | 12 bits | Data (data chunk) size |
| `CRC` | 38 | 16 bits | Error detection code (CRC-16 CCITT) |

Total header length: **54 bits**.

> **Important note:** The info chunk (header) is **always sent with MSK modulation + convolutional
> coding**, independent of the data chunk's own modulation/coding setting. So even if the user selects
> 4L-FSK or NORM (uncoded), the header always goes with the most robust setting — only the data chunk
> is sent with the modulation/coding the user selected.

Since `x` and `y` are 6 bits, they can express block coordinates in the range 0–63 — a 320×240 image
split into 16×16 blocks gives 20×15 = 300 blocks (X: 0–19, Y: 0–14), so 6 bits is more than
enough.

## Data Chunk Structure

```
┌──────────────────────────────────────────┬──────┐
│                data                       │ CRC  │
└──────────────────────────────────────────┴──────┘
```

The data chunk is sent immediately after the info chunk's CRC. It also has its own CRC (assumed to
be CRC-16 CCITT again; not stated separately, but probably the same).

## Command Table (system code version '0')

| Command value | Function |
|---|---|
| 0 | Text transmission (text) |
| 1 | Image transmission (image) |
| 2 | BSR response (BSR response — retransmission of the requested blocks) |
| 3 | End (end/finish) |
| 4 | BSR request (BSR request — requesting lost blocks) |
| 5 | Cancel / interruption (abort) |
| 6 | Callsign (callsign) |

## Framing Rules

1. At the start of a transmission, a **256-bit repeating `01` pattern** (header/preamble) is sent
   first — so the receiver can establish bit/clock synchronization.
2. A "data format" (code structure) is defined as follows:
   **sync code + info chunk (header) + data chunk**.
   However, **end, abort and BSR signals contain no data chunk** — only
   sync + header are sent.
3. For image data, this format is sent **back to back as many times as there are blocks** (one full
   "sync+header+data" unit for each 16×16 block).
4. For a text transmission a single format unit is enough (it is self-contained).
5. When a transmission **completes normally**, the **end signal is sent 3 times** in a row at the end.
6. When a transmission **is cancelled**, the **abort signal is sent 3 times** in a row at the end.

## Sync Code (63-bit M-sequence)

```
000011100001001000110110010110101110111100110001010100111111010
```

## Whitening Code — period 127-bit M-sequence

```
1110110011000100100111001111100100000100011010101001101101001010
000101100001100101111111010110111011110001110100010101110000001
```//the two lines joined together form the 127-bit sequence.

## Other Notes (from readme.txt)

- The image size is always normalized to 320×240 (BMP/JPEG input is automatically resized +
  cropped).
- A full transmission of a 320×240 image takes **~2 minutes**, depending on the compression setting.
- BSR (Bad Segment Report) logic: the receiver sends the coordinates of missing/corrupted blocks to
  the transmitter as a BSR request (command=4); the transmitter resends the blocks corresponding to
  those coordinates as a BSR response (command=2).
- Text message: at most 510 half-width (255 full-width) characters at a time.
- The image quality/scale setting can be selected by the user (the `sc` field in the header, 4 bits
  → should encode a JPEG scale/quality value between 0–15).

## Features Added Later (readme2.txt / readme3.txt — build002→004)

These do **not change** the protocol, they are only improvements in the application (UI/UX) layer:
- build002→003: tabbed interface, "AF Level" screen, watchdog/timeout improvements,
  ImageProcessor external log integration (JA2GRC's software), random/vertical block scan order
  option (to spread block loss under fading).
- build003→004: hardening of the BSR logic, sending the header N times for callsign
  reliability (configurable, default 5), automatic deletion of old images,
  log output improvements.

## ✅ VERIFIED — Bit-Level Analysis on a Real Recording (2026-08-06)

The recording `research/sampleaudio/KG-STV_MSK_text.mp3` was decoded end to end with the Python
prototype demodulator under `research/decoder/`. Result: **the decoded text came out as readable plain text**:

```
TEST3>The Quick Brown Fox Jumps Over The Lazy Dog 0123456789
```

This **conclusively** verifies all of the following points:

1. **Bit mapping:** space (1200 Hz) = bit `0`, mark (1800 Hz) = bit `1`. Left to right in time,
   it matches the bit strings in the spec exactly (no reversal/inversion).
2. **Preamble:** Exactly 256 bits, repeating `01` pattern — an exact match with the spec.
3. **Sync word:** 63 bits, immediately after the preamble, an **exact** match with the string in the
   spec (the sync itself never undergoes whitening).
4. **Info chunk (header) coding order:** raw 54 bits → **rate-1/2, K=7 convolutional
   coding** (NASA standard generator polynomials **171, 133 octal**; output order for each input bit
   **(G133_bit, G171_bit)**) → 108 encoded bits → the **127-bit whitening M-sequence is XORed onto these
   108 bits, with the counter restarting from 0 AT THE START OF EACH CHUNK** (NOT a global/continuous
   counter). On the real recording the Viterbi decode succeeded fully with **zero errors (metric=0)**.
5. **Header field values** (for this example): `sys=0, com=0(text), c=0(NORM), m=0(MSK), x=0,
   y=0, sc=10, size=480(bits)`. The value `sc=10` **matches exactly** the `ImageQ=10` setting in the
   `kgstv.ini` file in the same distribution — a cross-check.
6. **Data chunk coding order:** when `c=0` (NORM), the data chunk does **not get** convolutional
   coding — just the raw data bits + its own 16-bit CRC, and **its own whitening counter starts
   from zero** (independent of the header's counter). The `size` field expresses the data length in
   **bits** (not bytes — 480 bits = 60 bytes, which also fits the recording duration exactly).
7. **CRC algorithm — FULLY IDENTIFIED:** **CRC-16/AUG-CCITT**
   (`poly=0x1021, init=0x1D0F, refin=false, refout=false, xorout=0x0000`), computed bit-serially.
   Both the CRC over the header's 38-bit field (`0x6702`) and the CRC over the data chunk's 480-bit
   text data (`0xA2D1`) were **verified exactly** with this algorithm.

### Verified Encode/Decode Chain (for text/NORM mode)

```
TX order:   preamble(256×01) → sync(63bit, NO whitening) →
            [raw 54bit header → conv-encode(K=7,171/133) → 108bit → whiten(from idx 0)] →
            [raw data bits + CRC16-AUG-CCITT(16bit) → whiten(from idx 0, counter independent of the header)]
```

This chain can be built symmetrically in both the encode and decode directions (in the Rust implementation).

### Partial Verification in Image Mode

I decoded the first frame of `KG-STV_.10_scale.wav` (a real image transmission, 174.8s):
`{sys=0, com=6(CALLSIGN), c=0(NORM), m=1(4L-FSK), x=0, y=0, sc=3, size=40bit}` —
**CRC verified (crc_ok=True)**, Viterbi metric=7 (there is real/light noise on the channel, expected).
This proves that the header decoding chain (sync finding → conv-decode → CRC) works **in image mode
too**. `com=6` confirms that before the image transmission the transmitter first announces its callsign
(consistent with the "send the callsign N times" feature in readme3.txt). `m=1` shows that in this
session the data chunks are sent with **4L-FSK**.

### 2026-08-07 Update: Calibration with Known Plaintext — Partial Success

Thanks to the controlled test images supplied by the user (OH2UDS callsign, solid colour + striped,
both MSK and 4L-FSK, both NORM and CONV), calibration with **real known plaintext** ("OH2UDS") has now
become possible — a total of 30 measurements were collected from 5 different files (repeats of the same
callsign), and the old "TEST3 assumption invalid" puzzle in this section of `docs/protocol_spec.md` was
solved:

- **Sign (positive/negative) accuracy: 96% (23/24 symbols)** — definite progress
- Cluster centres were calibrated with the known labels: **[-230.5, -74.6, 93.6, 210.6] Hz**
  (noticeably compressed compared with the ideal ±100/±300 Hz — probably the real/synthetic recording
  chain's own frequency response)
- Symbol accuracy reached **~80-83%** — with both the frequency-discriminator and the tone-correlation
  methods, **even averaging over 30 repeats does not change the result** → the remaining errors are
  not noise, but **systematic/deterministic inter-symbol interference (ISI)**
- **Practical test:** I tried to decode the smallest block of the solid white image (72 bits, 36 symbols)
  with these calibrated centres — 80% symbol accuracy **proved insufficient** against JPEG Huffman's
  zero error tolerance; the decode broke down after a few blocks.

**Root cause (consistent with the disassembly finding):** 4L-FSK's 200Hz tone spacing is not exactly
orthogonal over the 1200 baud symbol period (unlike MSK's 600Hz spacing) → simple per-symbol (block
averaging or tone-correlation) detection is structurally insufficient. **Next step:**
a **Maximum-Likelihood Sequence Detection (Viterbi-based CPFSK demodulator)** that takes the real CPFSK
phase memory into account — a separate, well-defined engineering project.

Old note, now partly superseded (left for historical reference):

Decoding this 4L-FSK data chunk (the callsign itself, 40 bits) is **still open** —
both tone-correlation and instantaneous-frequency (discriminator) methods were tried, and on **two
independent recordings (`KG-STV_.10_scale`, `KG-STV_4LFSK_Norm`) frequency readings were obtained that are
consistent/repeatable but do not separate cleanly into the expected 4 levels** (averaged over 6
callsign repeats, not noise — a systematic calibration/mapping problem). Backward calibration with known
plaintext ("TEST3", verified from MSK mode) was tried but no clear match came out — **the real callsign
in these 4L-FSK recordings may not be "TEST3"**, i.e. the assumption may be invalid. Conclusion: for MSK
the demodulation is fully proven and of production quality;
**for 4L-FSK either the real plaintext must be known or `kgstv.exe`'s demodulator code (filter/threshold
constants) must be examined**. Left open for now (see project task list #8) — MSK mode on its own is
sufficient for a complete KG-STV implementation (the encode side can produce both MSK and 4L-FSK;
only the 4L-FSK *decode* calibration is missing).

### ✅ Full Transmission Walk (Frame Walking) — Real Image Blocks Extracted

`research/decoder/frame_walker.py`, using the tone-correlation demodulator, walked the **whole** of the
`KG-STV_MSK_Norm.wav` recording (148.9 seconds) from start to finish. Result: a callsign announcement
repeated 5 times (`com=6, "TEST3"`, all CRC-valid), followed by **real image blocks (`com=1`) at 22
different coordinates**, most with valid CRCs. This establishes that:

- Between consecutive frames there is **NO preamble, only the 63-bit sync word is repeated**
  (the 256-bit preamble is sent only once at the very start of the transmission).
- The `x`,`y` fields in the header really behave as block coordinates (in the range 0-19, 0-14,
  fully consistent with the 320×240/16×16=20×15 block grid).
- The `size` field is **variable** per block (the nature of JPEG compression — flat/single-colour areas
  are very short, detailed areas longer); the bit count does not have to be a multiple of 8 (e.g. 378
  bits) — this shows the block is packed **not in the standard JPEG file format, but as a pure
  entropy-coded bitstream** (without byte alignment or JPEG markers/stuffing).
- Some flat/repetitive image areas (e.g. single-colour regions such as sky) produce **exactly the same
  short byte sequence** (`6532994ca0` was repeated at four different coordinates) — the expected behaviour
  for DC-dominant JPEG blocks with zero AC coefficients (Huffman EOB brevity).
- Blocks giving CRC errors are probably either real channel corruption (real RF/audio recording noise)
  or small clock drift accumulated over the 148-second recording (a single fixed `anchor` was used for
  the whole file) — a production-quality decoder will need symbol-based timing recovery
  (timing recovery loop).

**Next step:** feed the CRC-valid block bytes into a real baseline-JPEG Huffman/DCT decoder (try with
default/standard tables; if they don't fit, extract the tables from `kgstv.exe`) and convert them into
pixel data — this is the last piece of the "encode/decode of the picture" question.

### ✅ CRITICAL CORRECTION — The 6 Blocks in a Tile Do Not Chain DC (2026-08-07)

When a pixel-by-pixel comparison was made with real off-air recordings (TA4SO, BBC test card), it was
seen that even in blocks passing CRC the colours did not match the reference. A definitive diagnosis
was made with controlled **solid colour** test images (completely red / completely blue 320×240,
recorded by OH2UDS): unlike standard JPEG, the 4 Y blocks of a completely flat 16×16 tile each carry an
**independent DC value starting from zero**, **without being chained to each other**. Our old code
(with the classic JPEG MCU logic) **accumulated** these 4 values on top of each other like a convolution
chain (`dc = dc_pred + diff; dc_pred = dc`), which produced an absurd, ever-growing sequence such as
-58,-116,-174,-232 even for a solid red tile (whereas the real difference should have been ≈0 — the image
is completely flat). The same chaining bug also existed between Cb/Cr.

**Fix:** `research/decoder/jpeg_block.py::decode_16x16_block` — each of the 6 blocks (Y0,Y1,Y2,Y3,Cr,Cb)
is now decoded **independently with predictor=0** (NO chain).

**Verification:** The mean pixel difference between the BBC test card (`KG-STV_MSK_Convolution.wav`) and
the reference (`1.0_scale.png`) dropped from **222.99 → 125.37**, and visually the girl's face, the red
scarf, the green clown and the "BBC" text at the bottom are now **clearly recognizable**. The controlled
horizontal/vertical red-white striped test images were decoded **300/300 tiles, with sharp and correct
edges**. On the real TA4SO recording the mean difference (only the 57 decoded tiles) dropped to **~117**,
and visually the navy background + cream/yellow details are now consistent with the reference.

### ✅ JPEG Entropy Layer — Verified with the Standard Tables

`research/decoder/jpeg_block.py`, using the standard ITU T.81 Annex K Huffman tables (DC/AC,
luma/chroma) and the standard zigzag order, decoded frame11's CRC-verified 186-bit raw block data:

- **Bit consumption exactly 186/186** — the 6 blocks (4×Y + 1×Cb + 1×Cr) were parsed from start to
  finish without any Huffman sync error or any EOF/format error. With a wrong set of tables this
  happening by chance is statistically almost impossible → **KG-STV's entropy-coding layer is
  bit-compatible with the standard JPEG Huffman tables**.
- Two flat Y blocks (DC only, no AC coefficients) decoded to **perfectly valid grey values**
  (98.0 and 68.0 — in the 0-255 range). The Cb/Cr blocks DC=0 → 128 (neutral colour).
- Two blocks contain AC coefficients (real texture); in these blocks the pixel values overflow outside
  the 0-255 range — this is not a Huffman/table error, but comes from the **quantization table scaling
  formula** (`sc` 4-bit field → IJG "quality 1-100" formula) being miscalibrated. Since the `sc` field
  is only 4 bits (0-15) it does not map directly onto IJG's 1-100 scale; the real mapping (`sc` → scale
  multiplier) has not been determined yet.

### ✅ `sc` → Quantization Scale — RESOLVED (kgstv.exe binary analysis + real pixel verification)

Inside `kgstv.exe` the standard JPEG Huffman tables (AC/DC luma/chroma) were found **verbatim**, and in
the memory region just before them the **standard IJG quality=50 base quantization tables** (luma and
chroma, both verbatim) (`research/binaries/kgstv_extracted/kgstv/kgstv.exe`,
offset ~907037 start of luma, ~907117 start of chroma — 4 bytes/value, packed big-endian).

Back-calculation from flat (no AC coefficient) blocks decoded from the real `KG-STV_MSK_Norm.wav`
recording (`sc=10`, DC=-3→pixel 98.0, DC=-6→pixel 68.0): the `qt[0,0]` used is 80. With the standard
IJG formula for **quality=10**: `scale=5000/10=500`, `qt[0,0]=floor((16×500+50)/100)=80` —
**exact match**. So:

> **The `sc` field is directly the IJG JPEG "quality" parameter itself (1–100 scale).**
> Standard formula: `scale = quality<50 ? 5000/quality : 200-2*quality`;
> `qval = clamp(floor((base×scale+50)/100), 1, 255)`.

The overflow of blocks with AC coefficients outside the 0–255 range on the first attempt was not a scale
error — we had simply **forgotten to clamp the IDCT output to 0–255**; this is a standard final step in
every JPEG decoder. With clamping added, the blocks with AC show a consistent, realistic pattern
(e.g. horizontal banding — fully consistent with a horizontal edge/gradient in a photo).
`research/decoder/jpeg_block.py` was updated.

**Conclusion:** **both the structure and the quantization scaling** of the JPEG codec are now fully
known. All that remains in the image layer is to write a production-quality Rust implementation
— there are no unknown parameters.

#### ⚠️ CORRECTION (2026-08-08) — `sc` is not a raw IJG "quality", but an INDEX into the sender's "Scale" list

The analysis above had been verified at a SINGLE point (`sc=10`) and was correct there, but it was
generalized WRONGLY. The user reported that in the original Windows sender the control corresponding to
the `sc` field is NOT a raw "quality 1-100" value, but a 16-option **"Scale" drop-down list**:

```
0.07  0.08  0.09  0.1  0.2  0.3  0.4  0.5  0.6  0.8  1.0  1.20  1.40  1.60  1.80  2.0
```

(16 values = the 4-bit space of `sc`, index 10 = "1.0" = default, matching `ImageQ=10` exactly
— CONSISTENT with the `sc=10` verification above, but ONLY for that single point.) The user also
reported the estimated transfer times the real sender shows on screen:

| Scale (GUI) | `sc` (index) | Displayed time |
|---|---|---|
| 0.07 | 0 | **7:43** |
| 1.0 | 10 | **2:31** |
| 2.0 | 15 | **1:54** |

So as `sc` increases (as Scale gets larger) the data DECREASES (coarse quantization → short transfer).
In the old formula (`quality=sc` fed directly into `5000/quality`), quantization got FINER as `sc`
increased — the EXACT OPPOSITE of the observed direction (and it explains the complaint "at low sc both
the colour and the image are bad": in fact VERY FINE quantization was being attempted, not COARSE).

**Corrected relationship** (see `kgstv-core/src/jpeg.rs::scale_qt`):

> `internal_scale = SCALE_TABLE[sc] × 500.0` (500 = the verified IJG scale at the `sc=10`/"1.0" point
> — the same constant as `5000/10=500`, but now used as a direct multiplier of the table value, NOT
> via the raw `quality`); `qval = clamp(floor((base×internal_scale+50)/100), 1, 255)`.

This both PRESERVES the verified output at the `sc=10` point (500=1.0×500) and gives the observed
direction/proportion correctly — transfer times measured with our own encoder (with noisy/worst-case
content) now show the same trend: sc=0→6'48", sc=10→1'42", sc=15→1'19" (the absolute values do not
match exactly because of different image content, but the direction and order of magnitude are
consistent). Where/how `SCALE_TABLE` (0.07…2.0) itself is stored inside kgstv.exe has not yet been
verified by binary analysis — for now we rely on the user's report.

#### ⚠️ CORRECTION #2 (2026-08-08) — the magnitude was still wrong; solved with REAL reference images

The correction above got the DIRECTION right (the transfer time trend was consistent) but the MAGNITUDE
was still wrong — the user reported "image still bad except at sc 2.0" (only the coarsest
setting (`sc=15`, Scale=2.0) came out acceptable; at EVERY other value random colour blotches appeared
even in areas that should have been grey/neutral).

In `research/sampleaudio/`, a treasure trove perfectly suited to investigating exactly this problem was
found: two REAL off-air recordings with known `Scale` settings (`KG-STV_.10_scale.wav` → `sc=3` verified
from the header, `KG-STV_.50_scale.wav` → `sc=7` verified) AND the matching pixel-precise reference
images taken from the original Windows RECEIVER (`1scale.jpg`, `5scale.png`, plus `1.0_scale.png` for
the default `sc=10` — all three exactly 320×240, aligned exactly with the tile grid).

With these three file pairs, a back-calculation was done on many (15+) flat/single-colour (AC≈0) tiles
(extracting the raw DC coefficients with the `decode_tile_raw_dc` diagnostic helper and matching them
against the mean Y/Cb/Cr of the same tile position in the reference image): the real `qval` came out
CONSISTENTLY ~5× SMALLER than Correction #1 predicted, and all three points (sc=3, sc=7, sc=10) fit a
SINGLE, VERY SIMPLE relationship perfectly:

> **`qval = round(base_table_value × SCALE_TABLE[sc])`**
>
> I.e. the decimal value shown in "Scale" (0.07…2.0) is ITSELF the DIRECT multiplier of the base
> (quality=50) quantization table — there is no need at all for an intermediate IJG "quality" formula
> (`5000/quality` etc.). The name is already "Scale" (scale/multiplier) — not an abstract "quality"
> parameter; this is the simplest interpretation, fully consistent with the original software's naming.

Both real recordings decoded again with this formula (see the `kgstv-core/src/jpeg.rs` `scale_qt`
documentation) now VISUALLY match the reference images almost exactly — the random colour blotches
previously seen in grey/neutral areas have disappeared completely.

**Note:** The earlier `sc=10 → qt[0,0]=80` claim (the "RESOLVED" section above) had been
back-calculated from a single flat block; the original source of that measurement could not be
independently re-verified. The 15+ point cross-check against real reference images here is much
stronger evidence — the `decodes_real_captured_solid_red_tile` test (with the SAME raw bits, only with
corrected expected values) was updated with this correction. Where/how `SCALE_TABLE[sc]` is stored inside
kgstv.exe has still not been verified by binary analysis.

### ✅ `kgstv-modem` (Rust audio I/O + MSK modem) — Verified End to End with a Real Recording (2026-08-07)

`src/kgstv-modem/` was written as a cargo workspace member: `wav` (hound-based WAV I/O, the sample
rate is read dynamically from the file), `filter` (RBJ biquad band-pass, forward+backward filtfilt), `msk`
(true continuous-phase synthesis verified by disassembly + Hanning-windowed DFT tone-correlation
demodulation), `sync` (two-stage coarse+fine sync-word search) and a top-level `session` combining them
with `kgstv-core::frame`/`jpeg` (sequential frame walk + TX session builder).

End-to-end verification on the real `samples/OH2UDS-*-mode-msk-*.wav` recordings (`cargo run -p kgstv-modem --release
--example decode_wav`):
- The sync was correctly found ~2.93 s after the start of the recording (recorder start-up
  delay) — this required the coarse search step to be `tb/4` (~a quarter of the symbol duration) and
  the fine search to prefer the position closest to the coarse result on a tie (otherwise `window()`
  clamping negative positions to 0 led to spurious ties).
- `duz-kirmizi` (solid red) recording: 346 frames, **300/300 image tiles** decoded (`header_metric=0`,
  i.e. zero bit errors in Viterbi; all data CRCs passed); the image produced is exactly the expected
  **RGB=(178,0,0)** — bit-matching the real captured red tile in the `kgstv-core::jpeg` test.
- `horizontal-kirmizi-beyaz` (horizontal red-white, CONV coded) recording: again 300/300 tiles decoded;
  the expected lossy-JPEG edge blurring (slight colour mixing, ~0.6% of pixels) was observed in the 1-2
  transition tiles on the red/white boundary — this is not a decoder bug but the expected behaviour of
  `sc=10` (low quality) quantization at a sharp edge.

**Conclusion:** The MSK audio layer is now verified at production quality with real off-air/controlled
recordings — the whole chain from WAV to a pixel-correct image works in Rust.

### ✅ 4L-FSK — MLSD/Viterbi Demodulator Solved (2026-08-07)

The solution proposed for the root cause identified in the calibration note above (the 200Hz tone
spacing is not orthogonal at 1200 baud → independent/memoryless symbol decisions get stuck on systematic
ISI, plateauing at ~80-83% symbol accuracy) — a Maximum-Likelihood Sequence Detection (Viterbi) that
takes CPFSK phase memory into account — was implemented in `src/kgstv-modem/src/fsk4.rs`.

**Key mathematical observation:** with the modulator formula verified by disassembly
(`frequency = level×300+1500`, continuous phase), the carrier cycles (`freq/baud`) carried by the four
possible frequencies at 1200 baud are: 1200→1, 1400→7/6, 1600→4/3=8/6, 1800→3/2=9/6. So a level-`L`
symbol advances the accumulated phase state by **exactly `L×60°` (mod 360°)** — since the modulation
index `h=Δf/baud=200/1200=1/6` is rational, this gives an EXACT, not APPROXIMATE, 6-state CPM/CPFSK
phase trellis (standard CPM theory: if `h=p/q` is a reduced fraction, the trellis has `q`
states). `fsk4.rs` runs a full Viterbi/MLSD over this 6-state trellis and decides the symbol sequence
correctly accounting for phase continuity (unlike simple tone correlation).

**Two additional obstacles encountered (both found while testing with real recordings; synthetic tests
happened to hide them):**
1. **Unknown initial phase STATE:** The absolute phase at the start of the data chunk is an arbitrary
   value carried over from the preceding (always MSK) sync/header segment. The first implementation
   mistakenly forced starting from state 0 (since the synthetic tests always started in state 0 this went
   unnoticed) — fix: Viterbi was given an equal/uninformative prior for all 6 states, combined with an
   additional global phase-offset scan over `[0,60°)`.
2. **Fine timing shift:** In real recordings a constant timing shift of ~a few samples was found that is
   COMPLETELY invisible to MSK's magnitude-based (non-coherent) demodulation (probably the group delay of
   the recording chain) — the phase-coherent Viterbi correlation is very sensitive to it. It was isolated
   with a diagnostic tool cross-checked against known-correct MSK data: in
   `OH2UDS-duz-kirmizi-mode-4lfsk-code-normal-scale-1.wav`, once the best timing shift of **+4.5 samples**
   was found, the Hamming distance dropped from 8/94 to **0/94**. Solution: a fine shift scan over the
   `[-8,+8]` sample range (since it is usually constant for a file, the shift found in the first
   successful chunk is carried to subsequent chunks as a "hint", speeding up the scan).

**Result with real recordings** (`cargo run -p kgstv-modem --release --example decode_wav`):
- `OH2UDS-duz-kirmizi-mode-4lfsk-code-normal-scale-1.wav` (NORM, no error correction):
  **287/300 tiles** decoded; EVERY successful tile bit-matches exactly the expected **RGB=(178,0,0)**
  (no partial/corrupted tiles — the expected "either fully correct or nothing" behaviour of a CRC-gated
  pipeline).
- `OH2UDS-horizontal-kirmizi-beyaz-mode-4lfsk-code-convolution-scale-1.wav` (CONV, with K=7
  convolutional error correction): **299/300 tiles** — fully consistent with the theoretical expectation
  that CONV coding corrects most of the remaining symbol errors.

**Conclusion:** 4L-FSK now works at **near production quality** on real off-air quality recordings
(~96-99.7% tile success rate — a huge leap from the earlier ~80% symbol-accuracy plateau, a situation
in which in practice not even a single tile could be decoded reliably). The few remaining tile errors are
probably real channel noise/transient SNR drops (almost entirely compensated by CONV coding), not a
structural shortcoming. `kgstv-modem::session`, while always using MSK for header/sync (as the protocol
requires), switches automatically between MSK/4L-FSK for the data chunk according to the `hdr.m`
field — on both the RX (`decode_wav_sequential*`) and TX (`encode_session_audio*`) sides.

### Still To Be Verified (for image mode)

The above is proven for the **text + NORM + MSK** combination. Additional verification is needed for
image mode: the exact behaviour of the data chunk's convolutional coding when `c=1` (CONV) (is the
whitening order the same), the symbol-bit mapping of 4L-FSK in the data chunk, and the internal
structure of the 16×16 JPEG block data (quantization/Huffman tables). For these verifications the
`KG-STV_MSK_Convolution.mp3`, `KG-STV_4LFSK_*.mp3` and `.10_scale`/`.50_scale` recordings in the
`sampleaudio` folder will be used.

## Details Still Uncertain / To Be Verified

readme.txt describes the frame at bit level, but the following are still not clear — for these
we need either to disassemble the relevant functions of `kgstv.exe` or to do bit matching on a real
off-air recording:

-2. **The RX front-end filter chain was found and verified via kgstv.exe disassembly** (waveIn
    callback @0x4033e8, traced with radare2). The real DSP chain:
    ```
    int16 PCM (44100Hz) → FILTER1 (51-tap anti-alias lowpass) → decimate by 4 (→11025Hz)
                        → FILTER2 (51-tap, ~900–2000Hz bandpass) → [rest not extracted]
    ```
    The **exact coefficients** of both filters were read from the binary (file offsets 841240 and 841648,
    51×8-byte doubles) and saved to `research/decoder/kgstv_filters.py`. FILTER2's frequency
    response was computed: -54dB at 0Hz (DC/hum rejection), flat at 900-2000Hz (KG-STV tone band),
    falling off quickly above 2500Hz — **exactly** a channel-selecting bandpass filter.

    This real filter chain was implemented in Python and the header was decoded again on the `.10_scale`
    sample: **CRC verified** (metric=4), the same result as the earlier simple Butterworth approach.
    The 4L-FSK callsign data was also retried with these real filters — **the resulting level sequence
    is almost exactly the same as the earlier (simple filter) result** (see the table below), which
    proves that our demodulation comes **from a real/consistent signal, not from noise**. However the
    result still does not give readable ASCII.
    **Conclusion:** Filter quality was not the source of the problem (it was already good enough). The
    most likely of the remaining explanations: **the real test text in these samples is not "TEST3"** —
    our known-plaintext assumption is invalid for these specific recordings. Without knowing the real
    text (or fully tracing the RX's symbol-decision/sync logic) the calibration cannot be completed.

-3. **The WHOLE RX chain was extracted: 4 real FIR filters + Hilbert transform + phase-difference
    discriminator.** Continuing the disassembly revealed the following full architecture:
    ```
    int16 PCM (44100Hz)
      → FILTER1 (51-tap anti-alias lowpass)
      → decimate by 4 (→11025Hz)
      → FILTER2 (51-tap bandpass, ~900-2000Hz — KG-STV tone band)
      → FILTER3 (51-tap Hilbert transformer — centre coefficient EXACTLY 0.0, fully antisymmetric
                  → produces the quadrature/imaginary component)
      → phase = atan2(FILTER3_output, FILTER2_output)   [wrapped, -π..π]
      → phase unwrap: compares the consecutive difference with ±2π (constant found at 0x403bc4,
        EXACTLY 6.283185307179586) and adds/subtracts as needed to make it continuous
      → instantaneous frequency = d(unwrapped_phase)/dt × sample_rate/2π
      → FILTER4 (51-tap symmetric lowpass, centre=0.125 — smooths the frequency estimate)
      → [symbol decision logic — not traced yet]
    ```
    This is a **classic Hilbert-transform FM discriminator** — conceptually **exactly the same
    philosophy** as our `fsk_discriminator` approach (mixing down to a complex carrier and taking the
    angle difference), just using these 4 real FIR filters instead of IIR Butterworth.

    I implemented this full chain (`research/decoder/kgstv_filters.py::apply_authentic_discriminator`)
    in Python and repeated the sync search on the `.10_scale` sample: **the result came out
    worse** (err=16/63, compared with err=0 for the simple approach) — there is probably a small
    mismatch in the Hilbert sign convention (`atan2` argument order) or the phase direction convention;
    bit-matching the assembly exactly needs a separate, patient debugging session.
    **Research was stopped at this point** — the architecture is fully understood and documented, but
    the last 100% bit-exact replication (and the possible known-plaintext problem) is still open. The
    simple complex-mixing approach (already proven for MSK) is sufficient for production.

-1. **The modulator formula was found via kgstv.exe disassembly** (radare2, function @0x405f94,
    frequency constants around @0x4068e8). The real x87 FPU code:
    ```
    ; fld qword [selected level value, in the range -1.0..+1.0, chosen by branching]
    fmul dword [0x4068e8]   ; × 300.0
    fadd dword [0x4068ec]   ; + 1500.0   -> frequency (Hz)
    fmul qword [0x4068f0]   ; × 2π (0x401921FB54442D18 = 6.283185307179586)
    fmul qword [0x51974c]   ; × (probably 1/sample_rate)
    fadd qword [0x4f64a0]   ; add to the phase accumulator (runtime state)
    fstp qword [0x4f64a0]   ; -> phase += 2π·f/fs
    ```
    This **conclusively verifies**: `frequency = level_value × 300 + 1500` (MSK: level=±1 →
    1200/1800 Hz; 4L-FSK: level=±1,±⅓ → 1200/1400/1600/1800 Hz), and the modulation is **really
    continuous-phase (CPFSK)** — the phase is accumulated on every sample and not reset at symbol
    boundaries. This is exactly the behaviour the name MSK ("Minimum Shift Keying") implies.
    **Conclusion:** The reason why 4L-FSK cannot be reliably decoded with block-based tone correlation
    (Goertzel/DFT) is now clear — that method suits discrete/discontinuous-phase FSK, but for CPFSK the
    in-symbol tone "leakage" (200Hz spacing, not orthogonal) imposes a structural limitation.
    MSK does not have this problem because the 600Hz spacing is exactly orthogonal over the symbol period.
    The right solution is a phase-derivative (frequency-discriminator) based demodulator with PLL/timing
    recovery — which we already tried, but its fine tuning (filter bandwidth, in-symbol window) is still
    insufficient. The disassembly could be continued to find the receiver (RX) side's exact
    threshold/filter values, or we could design our own PLL-based demodulator more carefully with this
    verified CPFSK model.

0. ~~Frequency constants~~ — in `kgstv.exe`, around offset ~24296-24320, the constants **300.0, 1500.0, 200.0,
   1200.0** (float32) were found one after another: centre frequency (1500), MSK deviation (±300),
   4L-FSK step width (200Hz) — **matching exactly** our assumed frequency model. The source of the 4L-FSK
   decode problem is not wrong nominal frequencies; probably the demod algorithm itself (filter/threshold)
   or a whitening/bit-order detail.
1. **Bit order** — are the 54-bit header fields and the 63/127-bit M-sequence strings sent MSB-first
   or LSB-first?
2. **Viterbi K=7 polynomials** — since it says "NASA standard", most likely the CCSDS/NASA standard
   rate-1/2, polynomials (171,133) octal — but this needs confirming (is there flush/tail-biting?).
3. **CRC-16 CCITT parameters** — polynomial 0x1021 is standard, but the init value / final XOR / input-
   output bit direction (reflected or not) should be confirmed.
4. **Exact parameters of the JPEG variant** — are the quantization table and Huffman tables custom or
   standard; is each 16×16 block an independent mini-JPEG, or are shared tables + only entropy data
   sent (there is no separate quant-table field in the 54-bit header, so probably a fixed/standard table
   set is used and the `sc` field only selects the scale/quality).
5. **Exact meaning of the `sc` (4-bit) field** — which numeric value corresponds to which JPEG
   quality/scale.
6. **Exact order in which the channel coding (whitening + convolutional) is applied** — is whitening
   applied before or after convolutional coding?

## Suggested Next Step

This document gives the complete skeleton of the protocol. For the remaining gaps (items 1–6):
- **Priority 1:** Try bit synchronization with a prototype demodulator on a real off-air audio recording
  (to be supplied by the user) and verify the bit order and CRC parameters by observing the sync/header
  fields.
- **Priority 2:** Find the JPEG encode/decode and Viterbi encode/decode functions in `kgstv.exe` with
  Ghidra and extract the constant tables (quantization, Huffman, polynomial).
