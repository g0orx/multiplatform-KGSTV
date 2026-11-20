# KG-STV — Research Notes

## Project Goal
To produce a rewrite of the Windows-only **KG-STV** program written by JJ0OBZ that is
**on-air compatible** with the original protocol and runs on Windows, Linux, macOS and (if
possible) iOS/Android. In other words, the goal is not just "a similar program" but a
bit-compatible implementation that can communicate with real KG-STV stations.

## What Is KG-STV
KG-STV is a digital SSTV (Slow-Scan Television) mode. It allows sending/receiving images and
short text messages by radio via a sound card (soundcard-based). Unlike analogue SSTV, it
compresses the image as JPEG and sends it in pieces using digital modulation.

- **Developer:** JJ0OBZ (Japan, amateur radio operator, "K.G.")
- **Image size:** 320×240 (the same as classic analogue SSTV modes)
- **Block structure:** The image is split into 16×16-pixel blocks, each of which is JPEG-compressed
  and sent separately. This way, even if the transmission is interrupted, the blocks received
  produce a "holey" but visible result in the image.
- **Error correction / retransmission requests:** The receiver reports corrupted/lost blocks to
  the transmitter with a feedback message called a **BSR (Bad Segment Report)**; the transmitter
  resends only those blocks.
- **Text messaging:** In addition to images, short text messages are also supported.

## Modulation Details
There are two modulation modes:

| Mode | Description | Speed | Note |
|---|---|---|---|
| **MSK** | Minimum-Shift Keying | 1200 Bd | Slower, more stable (resistant to fading) |
| **4LFSK** | 4-Level Frequency-Shift Keying | 1200 Bd (2 bits/symbol → effectively 2x data) | Faster, more sensitive to propagation impairments |

**MSK frequencies:** 0 → 1200 Hz, 1 → 1800 Hz
**4LFSK frequencies:** 00→1200 Hz, 01→1400 Hz, 10→1600 Hz, 11→1800 Hz

- **Bandwidth:** ~500–2500 Hz (varies with signal conditions)
- **Error-correcting code:** Viterbi convolutional coding, NASA standard **K=7**, optional
  (the user can also send without encoding)
- **Operation:** USB (Upper Sideband), on the amateur SSTV bands

## Known Frequencies / Bands
- 3733 kHz, 7173 kHz, 14233 kHz (HF SSTV bands)
- QO-100 via 10.489625 GHz (geostationary amateur transponder)

## State of the Software
- **Closed-source**, runs only on Windows.
- The only known receive/transmit program is JJ0OBZ's own software; no other implementation
  (including open source) appears to have been completed so far.
- The idea of writing a GNU Radio decoder block was discussed on `sstv@groups.io`, but no result
  was found.

## Missing / Unknown Technical Details (require reverse engineering)
No source found on the web currently contains **full protocol documentation**. Points that need
to be clarified for a bit-compatible implementation:

1. **Synchronization / preamble sequence** — how the receiver finds bit synchronization and block
   boundaries.
2. **Packet / header format** — what header information (block coordinates, checksum/CRC, mode
   information etc.) is added when each 16×16 block is sent.
3. **Exact parameters of the JPEG variant inside a 16×16 block** — is it standard baseline JPEG,
   or is a customized DCT/quantization table used?
4. **BSR (Bad Segment Report) packet format** — the bit-level structure of the feedback message.
5. **Text messaging framing** — how text packets are distinguished from image packets.
6. **Exact Viterbi K=7 parameters** — generator polynomials, puncturing (if any).

These details can only be extracted in the following ways:
- **Static analysis:** disassembling the KGSTV.exe binary (Ghidra/IDA) — constants,
  table data, encode/decode functions.
- **Dynamic analysis:** verifying the bit stream by performing spectrogram/demodulation analysis
  on off-air audio recordings (WAV) of real KG-STV transmissions.

## Sources
- [KG-STV - Signal Identification Wiki](https://www.sigidwiki.com/wiki/KG-STV)
- [M5AML - KG-STV, a digital SSTV system by JJ0OBZ](http://www.wells10000.plus.com/m5aml/kg-stv.htm)
- [What is KG-STV in Amateur Radio? - Johnson's Techworld](https://johnsonfrancis.org/techworld/what-is-kg-stv-in-amateur-radio/)
- [How to receive and decode KG-STV transmissions (sgcderek)](https://sgcderek.github.io/blog/kgstv-reception.html)
- [KG-STV MSK 1200Bd/600 via QO-100 (diario SWL I-56578)](http://i56578-swl.blogspot.com/2019/07/kg-stv-msk-1200bd600-via-qatar-oscar-100.html)
- [sstv@groups.io — open-source implementation of KG-STV?](https://groups.io/g/sstv/topic/open_source_implementation_og/80462530)

## Next Steps
See the project task list (task tracker) — next up:
1. Gather the reverse-engineering sources (binary + recordings)
2. Decide on the cross-platform technology stack
3. Actually extract the protocol
