#!/usr/bin/env python3
"""
Decodes a KG-STV WAV recording (e.g. one downloaded with a WebSDR's own recording feature) from
start to finish and produces a 320x240 PNG from the image blocks that could be decoded.

Usage:
    python3 decode_session.py <wav_file.wav> [output.png]

Notes:
- Image blocks in MSK mode (m=0) are fully decoded (framing + JPEG).
- 4L-FSK (m=1) blocks currently cannot be demodulated reliably (see docs/protocol_spec.md,
  "4L-FSK" section) — these blocks are left black (0,0,0) and the session continues.
- Missing/corrupted blocks stay black — the same as KG-STV's own visual behaviour.
"""
import sys
import os
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tone_demod import ToneClock, bandpass
from kgstv_decode import (load_mono, SYNC_WORD, dewhiten, swap_pairs, viterbi_decode,
                           parse_header, crc16_bitserial)
from jpeg_block import decode_16x16_block

PREAMBLE_LEN = 256
SYNC_LEN = 63
HEADER_ENC_LEN = 108
COMMANDS = {0: 'TEXT', 1: 'IMAGE', 2: 'BSR_RESP', 3: 'END', 4: 'BSR_REQ', 5: 'ABORT', 6: 'CALLSIGN'}
NO_DATA_COMMANDS = {3, 4, 5}
DATA_COMMANDS = {0, 1, 2, 6}

IMG_W, IMG_H = 320, 240
TILE = 16


def find_preamble_candidates(samples, sr, top_n=8):
    """In long (minutes-long) real recordings it is unknown WHERE the KG-STV transmission starts.
    The 256-bit '01' preamble produces a signal alternating at exactly 600Hz at 1200 baud; we search
    for it at the instantaneous-frequency (discriminator) output with a sliding Goertzel/FFT window
    to quickly find regions with strong 600Hz energy (candidate preamble positions)."""
    from kgstv_decode import fsk_discriminator
    from scipy.signal import stft
    inst = fsk_discriminator(samples, sr)
    win = max(64, int(sr * 0.05))  # ~50ms window
    f, t, Z = stft(inst, fs=sr, nperseg=win, noverlap=win // 2)
    idx600 = np.argmin(np.abs(f - 600))
    energy = np.abs(Z[idx600])
    order = np.argsort(energy)[::-1]
    candidates = []
    seen_times = []
    for o in order:
        tt = t[o]
        if any(abs(tt - s) < 0.3 for s in seen_times):
            continue
        seen_times.append(tt)
        candidates.append(int(tt * sr))
        if len(candidates) >= top_n:
            break
    return candidates, inst


def find_first_sync(clk, coarse_range, fine_span=3, fine_step=0.02):
    def sync_err(t0):
        bits = [clk.msk_bit(t0, PREAMBLE_LEN + i) for i in range(SYNC_LEN)]
        return sum(a != int(b) for a, b in zip(bits, (int(c) for c in SYNC_WORD)))

    best = None
    for t0 in coarse_range:
        e = sync_err(t0)
        if best is None or e < best[0]:
            best = (e, t0)
        if e == 0:
            break
    best2 = None
    for t0f in np.arange(best[1] - fine_span, best[1] + fine_span, fine_step):
        e = sync_err(t0f)
        if best2 is None or e < best2[0]:
            best2 = (e, t0f)
    return best2[1], best2[0]


def find_sync_wide(clk, samples, sr, verbose=True):
    """Finds the preamble/sync by scanning the whole recording: first spectral candidate positions,
    then a precise search around each candidate."""
    candidates, _ = find_preamble_candidates(samples, sr)
    if verbose:
        print(f"candidate preamble positions (samples): {candidates}")
    best_overall = None
    for c in candidates:
        lo = max(0, c - int(sr * 0.15))
        hi = c + int(sr * 0.15)
        step = max(1, int(sr / 8000))
        anchor, err = find_first_sync(clk, range(lo, hi, step), fine_span=step * 3, fine_step=0.05)
        if verbose:
            print(f"  candidate @{c} ({c/sr:.1f}s) -> anchor={anchor:.1f} err={err}")
        if best_overall is None or err < best_overall[1]:
            best_overall = (anchor, err)
        if err == 0:
            break
    return best_overall


class SyncLocator:
    """Helper that quickly finds the 63-bit sync word in long/noisy real recordings with a
    vectorized correlation. Computes the instantaneous-frequency array and its cumulative sum once for
    the whole file; then searches for the best match in any bit range (trying several phases)."""

    def __init__(self, samples, sr):
        from kgstv_decode import fsk_discriminator
        self.sr = sr
        self.Tb = sr / 1200.0
        self.inst = fsk_discriminator(samples, sr)
        self.cum = np.cumsum(self.inst)
        self.sync_pm1 = np.array([1 if c == '1' else -1 for c in SYNC_WORD], dtype=np.float64)
        self.idx_arr = np.arange(len(self.cum))

    def search(self, start_symbol, n_symbols, n_phases=24):
        """Searches for the best sync match in the symbol range [start_symbol, start_symbol+n_symbols)
        (relative to phase=0). Returns: (matches, symbol_idx, phase)."""
        best = None
        for k in range(n_phases):
            phase = k * self.Tb / n_phases
            idx = np.arange(start_symbol, start_symbol + n_symbols)
            a = phase + idx * self.Tb
            b = phase + (idx + 1) * self.Tb
            ia = np.clip(a, 0, len(self.cum) - 1)
            ib = np.clip(b, 0, len(self.cum) - 1)
            cum_a = np.interp(ia, self.idx_arr, self.cum)
            cum_b = np.interp(ib, self.idx_arr, self.cum)
            avgf = (cum_b - cum_a) / (b - a)
            bits_pm1 = np.where(avgf > 0, 1.0, -1.0)
            corr = np.correlate(bits_pm1, self.sync_pm1, mode='valid')
            bi = int(np.argmax(corr))
            matches = (63 + corr[bi]) / 2
            if best is None or matches > best[0]:
                best = (matches, idx[bi], phase)
        return best


def resync_local(clk, anchor, pos, span=6, step=0.05):
    """Checks whether the 63 bits immediately before pos really are the sync word, and locally
    fine-tunes the anchor to correct a small timing shift (drift).
    In long recordings (>1 minute) transmitter/receiver clock drift accumulates; this tracks it by
    making a small correction on every frame (a simple timing recovery/tracking loop)."""
    def err_at(a):
        bits = [clk.msk_bit(a, pos - SYNC_LEN + i) for i in range(SYNC_LEN)]
        return sum(x != int(y) for x, y in zip(bits, (int(c) for c in SYNC_WORD)))

    base_err = err_at(anchor)
    if base_err == 0:
        return anchor, base_err
    best = (base_err, anchor)
    for da in np.arange(-span, span, step):
        e = err_at(anchor + da)
        if e < best[0]:
            best = (e, anchor + da)
    return best[1], best[0]


def decode_header(clk, anchor, pos):
    raw = [clk.msk_bit(anchor, pos + i) for i in range(HEADER_ENC_LEN)]
    dew = dewhiten(raw, 0)
    swapped = swap_pairs(dew)
    hbits, metric, _ = viterbi_decode(swapped, 54)
    hdr = parse_header(hbits)
    crc_ok = crc16_bitserial(hbits[:38]) == hdr['crc']
    return hdr, metric, crc_ok


def find_best_header_anchor(clk, anchor_base, pos, tb, step=0.05):
    """resync_local only minimizes the bit errors of the sync word; this does not always coincide
    exactly with the anchor that minimizes the header's Viterbi metric (especially in real,
    noisy recordings). Here, starting from anchor_base and scanning over one full symbol period (tb),
    we find the anchor that DIRECTLY minimizes the header's own Viterbi metric —
    cheap for the 108-bit header (~tb/step tries per frame), and much more reliable on noisy real
    recordings."""
    best = None
    for da in np.arange(0, tb, step):
        a = anchor_base + da
        hdr, metric, crc_ok = decode_header(clk, a, pos)
        if best is None or metric < best[1] or (metric == best[1] and crc_ok):
            best = (a, metric, hdr, crc_ok)
        if crc_ok:
            break
    return best  # (anchor, metric, hdr, crc_ok)


def decode_data_bits(clk, anchor, pos, size_bits, conv):
    mult = 2 if conv else 1
    raw = [clk.msk_bit(anchor, pos + i) for i in range((size_bits + 16) * mult)]
    dew = dewhiten(raw, 0)
    if conv:
        swapped = swap_pairs(dew)
        bits, _, _ = viterbi_decode(swapped, size_bits + 16)
    else:
        bits = dew
    data_bits, crc_bits = bits[:size_bits], bits[size_bits:size_bits + 16]
    crc_val = int(''.join(map(str, crc_bits)), 2) if crc_bits else 0
    crc_ok = crc16_bitserial(data_bits) == crc_val
    return data_bits, crc_ok


def bits_to_ascii(bits):
    n = len(bits) - (len(bits) % 8)
    by = bytearray()
    for k in range(0, n, 8):
        v = 0
        for b in bits[k:k + 8]:
            v = (v << 1) | b
        by.append(v)
    return by.decode('ascii', errors='replace')


def ycbcr_tile_to_rgb(y_blocks, cb, cr):
    """y_blocks: [Y0,Y1,Y2,Y3] (top-left, top-right, bottom-left, bottom-right), each 8x8.
    cb, cr: 8x8 (covering the whole 16x16 tile, needs 2x upsampling)."""
    Y = np.zeros((16, 16))
    Y[0:8, 0:8] = y_blocks[0]
    Y[0:8, 8:16] = y_blocks[1]
    Y[8:16, 0:8] = y_blocks[2]
    Y[8:16, 8:16] = y_blocks[3]
    Cb = np.kron(cb, np.ones((2, 2)))
    Cr = np.kron(cr, np.ones((2, 2)))
    R = Y + 1.402 * (Cr - 128)
    G = Y - 0.344136 * (Cb - 128) - 0.714136 * (Cr - 128)
    B = Y + 1.772 * (Cb - 128)
    rgb = np.stack([R, G, B], axis=-1)
    return np.clip(rgb, 0, 255).astype(np.uint8)


def decode_frame_at(clk, phase, pos, tb):
    """Tries to decode the header at pos (and the data chunk, if any) independently.
    Does NOT assume a sequential 'walk' — each candidate is decoded on its own, trusting the
    header's own x/y/size fields. In noisy/fragmented real recordings this prevents one frame's
    failure from also dropping all subsequent frames.
    Returns: None (header could not be decoded) or a result dict."""
    anchor, metric, hdr, crc_ok = find_best_header_anchor(clk, phase, pos, tb)
    if not crc_ok:
        return None
    data_start = pos + HEADER_ENC_LEN
    result = {'anchor': anchor, 'metric': metric, 'hdr': hdr, 'pos': pos}
    if hdr['com'] in NO_DATA_COMMANDS:
        result['has_data'] = False
        return result
    result['has_data'] = True
    conv = bool(hdr['c'])
    if hdr['com'] in (1, 2) and hdr['m'] == 1:
        result['data_crc_ok'] = False
        result['note'] = "4L-FSK — demod not calibrated"
        return result
    data_bits, dcrc_ok = decode_data_bits(clk, anchor, data_start, hdr['size'], conv)
    result['data_bits'] = data_bits
    result['data_crc_ok'] = dcrc_ok
    return result


def place_image_tile(image, got_block, hdr, data_bits):
    """Converts the decode_16x16_block output YCbCr->RGB and places it in the image.
    Returns True if placed successfully."""
    try:
        results, _ = decode_16x16_block(data_bits, quality=hdr['sc'])
    except Exception:
        return False
    y_blocks = [px for c, px in results if c == 'Y']
    cb = next((px for c, px in results if c == 'Cb'), None)
    cr = next((px for c, px in results if c == 'Cr'), None)
    if len(y_blocks) != 4 or cb is None or cr is None:
        return False
    rgb = ycbcr_tile_to_rgb(y_blocks, cb, cr)
    px0, py0 = hdr['x'] * TILE, hdr['y'] * TILE
    if px0 + TILE > IMG_W or py0 + TILE > IMG_H:
        return False
    image[py0:py0 + TILE, px0:px0 + TILE] = rgb
    got_block[hdr['y'], hdr['x']] = True
    return True


def decode_long_recording(clk, data, sr, out_path):
    """For long/noisy real recordings: finds strong sync candidates (>=55/63 matches) across the
    whole file and tries to decode EACH ONE INDEPENDENTLY — assumes no order or
    continuity. Each successful IMAGE/BSR frame is placed at its own x,y coordinates."""
    Tb = sr / 1200.0
    locator = SyncLocator(data, sr)
    n_symbols = int((len(locator.cum) - Tb) / Tb)
    print(f"scanning the whole file ({n_symbols} symbols, may take a few minutes)...")

    all_candidates = []
    for k in range(24):
        phase = k * Tb / 24
        idx = np.arange(n_symbols)
        a = phase + idx * Tb
        b = phase + (idx + 1) * Tb
        ia = np.clip(a, 0, len(locator.cum) - 1)
        ib = np.clip(b, 0, len(locator.cum) - 1)
        cum_a = np.interp(ia, locator.idx_arr, locator.cum)
        cum_b = np.interp(ib, locator.idx_arr, locator.cum)
        avgf = (cum_b - cum_a) / (b - a)
        bits_pm1 = np.where(avgf > 0, 1.0, -1.0)
        corr = np.correlate(bits_pm1, locator.sync_pm1, mode='valid')
        good = np.where(corr >= 55)[0]
        for g in good:
            all_candidates.append((int((63 + corr[g]) / 2), int(g), phase))
    all_candidates.sort(key=lambda c: -c[0])
    print(f"{len(all_candidates)} strong sync candidates found (before dedup)")

    image = np.zeros((IMG_H, IMG_W, 3), dtype=np.uint8)
    got_block = np.zeros((IMG_H // TILE, IMG_W // TILE), dtype=bool)
    seen_bins = set()
    n_header_ok = n_image = n_crc_ok = n_decoded = n_fsk4_skipped = 0
    texts = []
    best_tile = {}  # (x,y) -> (metric, crc_ok) — if a tile is seen more than once, keep the best

    for matches, sidx, phase in all_candidates:
        key = sidx // 15  # deduplicate nearby candidates
        if key in seen_bins:
            continue
        seen_bins.add(key)
        pos = sidx + SYNC_LEN
        res = decode_frame_at(clk, phase, pos, Tb)
        if res is None:
            continue
        n_header_ok += 1
        hdr = res['hdr']
        t = sidx * Tb / sr
        if not res['has_data']:
            print(f"t={t:6.1f}s: {COMMANDS.get(hdr['com'],'?')}")
            continue
        if hdr['com'] in (1, 2):
            n_image += 1
            if hdr.get('m') == 1:
                n_fsk4_skipped += 1
                continue
            if res['data_crc_ok']:
                n_crc_ok += 1
                # NOTE: 'best-effort' (placing even with a CRC error) was tried, but in NORM (c=0)
                # data a single bit error completely breaks the JPEG Huffman stream (you get noise,
                # not an image) — so only CRC-valid blocks are placed.
                # This is the natural limit of passive listening on a real channel without BSR.
                xy = (hdr['x'], hdr['y'])
                if xy not in best_tile:
                    if place_image_tile(image, got_block, hdr, res['data_bits']):
                        best_tile[xy] = True
                        n_decoded += 1
                        print(f"t={t:6.1f}s: {COMMANDS[hdr['com']]} tile=({hdr['x']:2d},{hdr['y']:2d}) OK")
        elif hdr['com'] in (0, 6):
            if res['data_crc_ok']:
                txt = bits_to_ascii(res['data_bits'])
                texts.append((t, COMMANDS[hdr['com']], txt))
                print(f"t={t:6.1f}s: {COMMANDS[hdr['com']]} text={txt!r}")

    print(f"\n--- Summary ---")
    print(f"{n_header_ok} headers decoded from {len(all_candidates)} sync candidates")
    print(f"{n_image} image frames, {n_crc_ok} CRC-valid, {n_decoded} decoded to pixels")
    if n_fsk4_skipped:
        print(f"{n_fsk4_skipped} blocks skipped because they are 4L-FSK")
    print(f"tiles filled: {got_block.sum()}/{got_block.size}")
    if texts:
        print("Texts read:")
        for t, cmd, txt in texts:
            print(f"  t={t:.1f}s [{cmd}] {txt!r}")

    save_image(image, out_path)


def save_image(image, out_path):
    try:
        from PIL import Image
        Image.fromarray(image).save(out_path)
        print("saved:", out_path)
    except ImportError:
        np.save(out_path + ".npy", image)
        print("Pillow not found, raw array saved:", out_path + ".npy")
        print("To install: pip install pillow")


def decode_short_recording(clk, bp, sr, out_path):
    """For clean/short recordings (see the sampleaudio tests): after a single sync is found,
    walks sequentially (end of one frame = start of the next) — relies on continuity."""
    Tb = sr / 1200.0
    search_range = range(0, min(8000, max(1, len(bp) - 400)), 2)
    anchor, err = find_first_sync(clk, search_range)
    pos = PREAMBLE_LEN + SYNC_LEN
    print(f"anchor={anchor:.2f}  pos={pos}  preamble+sync errors={err}/63")

    image = np.zeros((IMG_H, IMG_W, 3), dtype=np.uint8)
    got_block = np.zeros((IMG_H // TILE, IMG_W // TILE), dtype=bool)
    n_image = n_crc_ok = n_decoded = n_fsk4_skipped = 0
    consecutive_fail = 0

    for i in range(3000):
        if pos + HEADER_ENC_LEN > len(bp):
            print(f"frame{i}@{pos}: audio ended, stopping.")
            break
        anchor2, metric, hdr, crc_ok = find_best_header_anchor(clk, anchor, pos, Tb)
        if crc_ok:
            anchor = anchor2
        if not crc_ok:
            consecutive_fail += 1
            print(f"frame{i}@{pos}: header CRC error (metric={metric}) — "
                  f"{'stopping' if consecutive_fail >= 5 else 'skipping and retrying'}")
            if consecutive_fail >= 5:
                break
            pos += 300
            continue
        consecutive_fail = 0

        data_start = pos + HEADER_ENC_LEN
        cmd = COMMANDS.get(hdr['com'], '?')
        if hdr['com'] in NO_DATA_COMMANDS:
            print(f"frame{i}@{pos}: {cmd}")
            pos = data_start + SYNC_LEN
            continue

        conv = bool(hdr['c'])
        note = ""
        if hdr['com'] in (1, 2):
            n_image += 1
            if hdr['m'] == 1:
                n_fsk4_skipped += 1
                note = "4L-FSK — demod not calibrated, skipping"
                data_bits, dcrc_ok = [], False
            else:
                data_bits, dcrc_ok = decode_data_bits(clk, anchor, data_start, hdr['size'], conv)
                if dcrc_ok:
                    n_crc_ok += 1
                    if place_image_tile(image, got_block, hdr, data_bits):
                        n_decoded += 1
                        note = "OK"
                    else:
                        note = "missing JPEG block"
            print(f"frame{i}@{pos}: {cmd} tile=({hdr['x']:2d},{hdr['y']:2d}) sc={hdr['sc']:2d} "
                  f"size={hdr['size']:4d}b crc_ok={dcrc_ok}  {note}")
        elif hdr['com'] in (0, 6):
            data_bits, dcrc_ok = decode_data_bits(clk, anchor, data_start, hdr['size'], conv)
            print(f"frame{i}@{pos}: {cmd} crc_ok={dcrc_ok} text={bits_to_ascii(data_bits)!r}")

        enc_len = (hdr['size'] + 16) * (2 if conv else 1)
        pos = data_start + enc_len + SYNC_LEN

    print(f"\n--- Summary ---")
    print(f"{n_image} image frames seen, {n_crc_ok} CRC-valid, {n_decoded} decoded to pixels")
    if n_fsk4_skipped:
        print(f"{n_fsk4_skipped} blocks skipped because they are 4L-FSK (see docs/protocol_spec.md)")
    print(f"tiles filled: {got_block.sum()}/{got_block.size}")
    save_image(image, out_path)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    wav_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) > 2 else "decoded_image.png"

    sr, data = load_mono(wav_path)
    print(f"loaded: {wav_path}  sr={sr:.0f}Hz  duration={len(data) / sr:.1f}s")

    bp = bandpass(data, sr)
    clk = ToneClock(bp, sr)

    if len(data) / sr > 5.0:
        decode_long_recording(clk, data, sr, out_path)
    else:
        decode_short_recording(clk, bp, sr, out_path)


if __name__ == "__main__":
    main()
