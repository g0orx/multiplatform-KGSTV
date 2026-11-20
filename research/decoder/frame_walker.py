"""
KG-STV frame walker — walks an entire transmission (callsign + image blocks) and
decodes each frame's header and (if present) data chunk, verifying them with CRC.

Usage: python3 frame_walker.py <wav_path> <initial_anchor_estimate>
First finds the sync word with tone_demod.ToneClock and fine-tunes it, then walks.

Findings: see docs/protocol_spec.md
"""
import sys
import numpy as np

sys.path.insert(0, '.')
from tone_demod import ToneClock, bandpass
from kgstv_decode import (load_mono, SYNC_WORD, dewhiten, swap_pairs, viterbi_decode,
                           parse_header, crc16_bitserial)

PREAMBLE_LEN = 256
SYNC_LEN = 63
HEADER_ENC_LEN = 108  # 54 raw bits * rate-1/2

COMMANDS = {0: 'TEXT', 1: 'IMAGE', 2: 'BSR_RESP', 3: 'END', 4: 'BSR_REQ', 5: 'ABORT', 6: 'CALLSIGN'}
NO_DATA_COMMANDS = {3, 4, 5}


def find_first_sync(clk, coarse_range, fine_span=3, fine_step=0.05):
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


def bits_to_bytes(bits):
    n = len(bits) - (len(bits) % 8)
    out = bytearray()
    for i in range(0, n, 8):
        v = 0
        for b in bits[i:i + 8]:
            v = (v << 1) | b
        out.append(v)
    return bytes(out)


def decode_header(clk, anchor, start_bit):
    raw = [clk.msk_bit(anchor, start_bit + i) for i in range(HEADER_ENC_LEN)]
    dew = dewhiten(raw, 0)
    swapped = swap_pairs(dew)
    hbits, metric, _ = viterbi_decode(swapped, 54)
    hdr = parse_header(hbits)
    crc_ok = crc16_bitserial(hbits[:38]) == hdr['crc']
    return hdr, metric, crc_ok


def decode_data(clk, anchor, start_bit, size_bits, conv, modulation):
    """modulation: 0=MSK (2-level). 4L-FSK not yet wired in here."""
    mult = 2 if conv else 1
    raw = [clk.msk_bit(anchor, start_bit + i) for i in range((size_bits + 16) * mult)]
    dew = dewhiten(raw, 0)
    if conv:
        swapped = swap_pairs(dew)
        bits, _, _ = viterbi_decode(swapped, size_bits + 16)
    else:
        bits = dew
    data_bits, crc_bits = bits[:size_bits], bits[size_bits:size_bits + 16]
    crc_val = int(''.join(map(str, crc_bits)), 2)
    crc_ok = crc16_bitserial(data_bits) == crc_val
    return data_bits, crc_ok


def walk(wav_path, coarse_range=range(0, 6000, 2), max_frames=1000, verbose=True):
    sr, data = load_mono(wav_path)
    bp = bandpass(data, sr)
    clk = ToneClock(bp, sr)
    anchor, err = find_first_sync(clk, coarse_range)
    if verbose:
        print(f"anchor={anchor} sync_err={err}")

    pos = PREAMBLE_LEN + SYNC_LEN
    frames = []
    image_blocks = {}
    for i in range(max_frames):
        hdr, metric, crc_ok = decode_header(clk, anchor, pos)
        if not crc_ok:
            if verbose:
                print(f"frame{i}@{pos}: header CRC fail, stopping. hdr={hdr} metric={metric}")
            break
        data_start = pos + HEADER_ENC_LEN
        rec = dict(pos=pos, hdr=hdr, metric=metric, cmd=COMMANDS.get(hdr['com'], '?'))
        if hdr['com'] not in NO_DATA_COMMANDS:
            conv = bool(hdr['c'])
            data_bits, dcrc_ok = decode_data(clk, anchor, data_start, hdr['size'], conv, hdr['m'])
            rec['data_bytes'] = bits_to_bytes(data_bits)
            rec['data_crc_ok'] = dcrc_ok
            if hdr['com'] == 1:
                image_blocks[(hdr['x'], hdr['y'])] = rec['data_bytes'] if dcrc_ok else None
            enc_len = (hdr['size'] + 16) * (2 if conv else 1)
            pos = data_start + enc_len + SYNC_LEN
        else:
            pos = data_start + SYNC_LEN
        frames.append(rec)
        if verbose:
            extra = ""
            if 'data_bytes' in rec:
                extra = f" data_crc_ok={rec['data_crc_ok']} bytes[:8]={rec['data_bytes'][:8].hex()}"
            print(f"frame{i}@{rec['pos']}: {rec['cmd']} {hdr} metric={metric}{extra}")
    return frames, image_blocks


if __name__ == '__main__':
    path = sys.argv[1] if len(sys.argv) > 1 else 'sampleaudio/wav/KG-STV_MSK_Norm.wav'
    frames, blocks = walk(path)
    ok = sum(1 for v in blocks.values() if v is not None)
    print(f"\n{len(blocks)} unique image block coords seen, {ok} CRC-valid")
