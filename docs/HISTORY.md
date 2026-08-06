# KG-STV: Origin and Background

## What KG-STV Is

KG-STV is a digital slow-scan television (SSTV) mode and software application for amateur
radio, created by an operator known by the callsign **JJ0OBZ**, who signs the software as
**"K.G."**, based in Niigata Prefecture, Myōkō City, Japan. Unlike classic analogue SSTV modes
(Martin, Scottie, Robot36, etc.), which encode a picture as a continuously varying audio tone
that traces brightness line by line, KG-STV takes a fully digital approach: it compresses an
image with JPEG-style encoding, splits it into small 16×16-pixel blocks, and transmits each
block as an independently error-checked digital packet over a normal SSB or FM voice channel,
using nothing more than a computer's sound card and a serial (RS-232) PTT connection to the
radio — the same kind of interface hobbyists already used for other ham radio digital modes.

The author built KG-STV purely out of personal interest in digital image transmission,
and said so candidly in the software's own documentation: it was an experiment, released so
other amateurs could try it, report back, and help refine it — not a polished commercial
product. That spirit shows in the design: a Bad Segment Report (BSR) mechanism lets a
receiving station ask the sender to retransmit just the blocks that were lost or corrupted,
so a full picture can still resolve even through a noisy or fading HF path, and even a
transmission that only got partially received still shows most of the image rather than
nothing at all.

## How It Spread

KG-STV found a real, if niche, following in the SSTV and digital-modes community. Off-air
logs and blog posts from the late 2010s show activity on the classic HF SSTV calling
frequencies (3733 kHz, 7173 kHz, 14233 kHz) and — notably — over the QO-100 geostationary
amateur satellite transponder (10.489625 GHz), where a stable, near-noise-free channel let
operators push KG-STV's higher-fidelity 4-level FSK mode and enjoy sharper, more reliable
image transfers than HF weather would normally allow. Stations logged exchanging KG-STV
images include callsigns such as DD4YR, DL9DAC, IZ5ILX, G3YPQ, SV8CS, and IZ1ERR, among
others — a small international group of enthusiasts who kept the mode alive by using it.

## The Documentation Problem

Here is where KG-STV became difficult for the wider ham radio world to adopt: **the software
and its documentation were written by and for a Japanese-speaking audience, and the program
itself only ever ran on Windows.** The installation archive ships three successive readme
files (`readme.txt`, `readme2.txt`, `readme3.txt`, covering versions 1.0.1 build 002 through
004) written entirely in Japanese, encoded in Shift-JIS — a character encoding that, opened
naively on a non-Japanese system, renders as unreadable garbage rather than legible text.
Critically, it is *this* Japanese-only readme that contains the software's actual technical
transmission specification: exact modulation frequencies, the bit-level frame structure,
error-correction scheme, and synchronization sequences — information that, as far as could
be determined, was never independently republished in English in a complete, technically
usable form. A shortcut link in the archive (`KG-STV English Tech info.URL`) hints that the
author or the community once tried to bridge that language gap, but no authoritative English
protocol reference equivalent to the Japanese original was found circulating publicly.

Compounding this, KG-STV was written for the classic Windows desktop environment of its era
(the shipped binary dates to 2010, built with a Borland/Embarcadero Delphi or C++Builder
toolchain) and never had a Linux, macOS, or mobile counterpart. So even an English-speaking
operator who could puzzle through the interface by trial and error was still locked out
unless they ran Windows.

## Why This Project Exists

The combination of those two barriers — a Windows-only binary, and a core specification
locked inside Japanese-language documentation — meant that KG-STV, despite being a genuinely
interesting and usable digital SSTV mode with an active (if small) real-world user base,
was effectively unreachable for a large part of the amateur radio community: Linux and macOS
users, mobile operators, and anyone who did not read Japanese and could not run the original
Windows executable.

This project's purpose is to fix that: to translate and reverse-engineer JJ0OBZ's original
work faithfully — starting from the Japanese readme and validating every claim against real
off-air recordings, bit by bit — and to re-implement KG-STV as a modern, cross-platform,
open, English-documented tool that can still talk to the original Windows software and the
existing on-air KG-STV community, while finally being available on Windows, Linux, macOS,
and, if feasible, iOS and Android.

## Where Things Stand

The framing, error-correction (rate-1/2 K=7 convolutional code), whitening sequence, CRC
algorithm, and MSK demodulation have all been independently confirmed by decoding real
captured KG-STV transmissions from scratch, down to the bit level, and re-implemented in a
portable Rust core. The JPEG image layer has been confirmed to use standard Huffman entropy
tables. Two details remain open: the exact quantization-scale mapping for JPEG blocks, and
reliable calibration of the 4-level FSK demodulator. Full technical detail lives in
[`protocol_spec.md`](protocol_spec.md).
