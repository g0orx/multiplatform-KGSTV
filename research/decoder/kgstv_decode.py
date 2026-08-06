"""
KG-STV prototip demodülatör/decoder — ters mühendislik doğrulaması için.
Sadece analiz amaçlı; performans için optimize edilmemiştir.

Doğrulanmış protokol parametreleri: bkz. docs/protocol_spec.md
"""
import numpy as np
from scipy.io import wavfile
from scipy import signal

SR_TARGET = 44100
BAUD = 1200.0

SYNC_WORD = "000011100001001000110110010110101110111100110001010100111111010"
WHITENING = ("1110110011000100100111001111100100000100011010101001101101001010"
             "000101100001100101111111010110111011110001110100010101110000001")
WBITS = [int(c) for c in WHITENING]
PREAMBLE_LEN = 256

K = 7

def poly_taps(oct_str):
    v = int(oct_str, 8)
    return [(v >> (K - 1 - i)) & 1 for i in range(K)]

G1 = poly_taps('171')
G2 = poly_taps('133')


def load_mono(path):
    sr, data = wavfile.read(path)
    data = data.astype(np.float64)
    if data.ndim > 1:
        data = data.mean(axis=1)
    data -= data.mean()
    return sr, data


def fsk_discriminator(data, sr, fc=1500.0, lp_cutoff=900.0):
    t = np.arange(len(data)) / sr
    bb = data * np.exp(-1j * 2 * np.pi * fc * t)
    b, a = signal.butter(4, lp_cutoff / (sr / 2), btype='low')
    bb = signal.filtfilt(b, a, bb)
    inst = bb[1:] * np.conj(bb[:-1])
    inst_freq = np.angle(inst) * sr / (2 * np.pi)
    win = np.ones(5) / 5
    return np.convolve(inst_freq, win, mode='same')


class BitClock:
    """Bit-level average-frequency sampler using cumulative-sum interpolation."""
    def __init__(self, inst_freq, sr, baud=BAUD):
        self.cum = np.cumsum(inst_freq)
        self.Tb = sr / baud

    def _interp(self, x):
        i0 = int(np.floor(x))
        frac = x - i0
        if i0 < 0:
            return self.cum[0]
        if i0 + 1 >= len(self.cum):
            i0 = len(self.cum) - 2
        return self.cum[i0] * (1 - frac) + self.cum[i0 + 1] * frac

    def avg_freq(self, t0, i):
        a = t0 + i * self.Tb
        b = t0 + (i + 1) * self.Tb
        return (self._interp(b) - self._interp(a)) / (b - a)

    def bit(self, t0, i, thresholds=None):
        f = self.avg_freq(t0, i)
        if thresholds is None:
            return 1 if f > 0 else 0
        # nearest-level quantization for 4L-FSK: thresholds = sorted deviation centers
        return min(range(len(thresholds)), key=lambda k: abs(f - thresholds[k]))

    def bits(self, t0, n):
        return [self.bit(t0, i) for i in range(n)]


def find_anchor_and_sync(clk, search_range=range(0, 6000)):
    """Coarse+fine search for preamble/sync alignment. Returns (anchor_t0, sync_bit_index)."""
    best = None
    for t0 in search_range:
        bits = clk.bits(t0, PREAMBLE_LEN)
        alt0 = [i % 2 for i in range(PREAMBLE_LEN)]
        err = sum(b != a for b, a in zip(bits, alt0))
        if best is None or err < best[0]:
            best = (err, t0)
        if err == 0:
            break
    err, t0 = best
    # refine: extend bits and locate sync word by string search
    n = 3000
    bits = clk.bits(t0, n)
    s = ''.join(map(str, bits))
    pos = s.find(SYNC_WORD)
    return t0, pos


def dewhiten(bits, reset_index=0):
    return [b ^ WBITS[(i + reset_index) % 127] for i, b in enumerate(bits)]


def viterbi_decode(rx_pairs_swapped, nbits, start_state=0):
    """rx_pairs_swapped: flat list of length 2*nbits, already reordered as (o2,o1) per bit."""
    nstates = 1 << (K - 1)
    INF = float('inf')
    pm = [[INF] * nstates]
    pm[0][start_state] = 0
    prev = [[None] * nstates for _ in range(nbits + 1)]
    inp = [[None] * nstates for _ in range(nbits + 1)]
    for t in range(nbits):
        npm = [INF] * nstates
        r1, r2 = rx_pairs_swapped[2 * t], rx_pairs_swapped[2 * t + 1]
        for s in range(nstates):
            if pm[t][s] == INF:
                continue
            for bit in (0, 1):
                reg = [bit] + [(s >> i) & 1 for i in range(K - 1)]
                o1 = sum(a * b for a, b in zip(reg, G1)) % 2
                o2 = sum(a * b for a, b in zip(reg, G2)) % 2
                ns = ((s << 1) | bit) & (nstates - 1)
                m = pm[t][s] + (o1 != r1) + (o2 != r2)
                if m < npm[ns]:
                    npm[ns] = m
                    prev[t + 1][ns] = s
                    inp[t + 1][ns] = bit
        pm.append(npm)
    es = min(range(nstates), key=lambda s: pm[nbits][s])
    bits = []
    s = es
    for t in range(nbits, 0, -1):
        bits.append(inp[t][s])
        s = prev[t][s]
    bits.reverse()
    return bits, pm[nbits][es], es


def swap_pairs(bits):
    out = []
    for i in range(0, len(bits), 2):
        out += [bits[i + 1], bits[i]]
    return out


def bits_to_bytes(bits):
    out = bytearray()
    for i in range(0, len(bits) - 7, 8):
        v = 0
        for b in bits[i:i + 8]:
            v = (v << 1) | b
        out.append(v)
    return bytes(out)


def crc16_bitserial(bits, poly=0x1021, init=0x1D0F):
    reg = init
    for bit in bits:
        top = (reg >> 15) & 1
        newtop = top ^ bit
        reg = (reg << 1) & 0xFFFF
        if newtop:
            reg ^= poly
        reg &= 0xFFFF
    return reg


def parse_header(bits54):
    def v(b):
        r = 0
        for x in b:
            r = (r << 1) | x
        return r
    return dict(
        sys=v(bits54[0:4]), com=v(bits54[4:8]), c=bits54[8], m=bits54[9],
        x=v(bits54[10:16]), y=v(bits54[16:22]), sc=v(bits54[22:26]),
        size=v(bits54[26:38]), crc=v(bits54[38:54]),
    )


def decode_header_at(clk, anchor, info_start_bit):
    """Decode the 54-bit info chunk starting at absolute bit index info_start_bit."""
    raw = [clk.bit(anchor, info_start_bit + i) for i in range(108)]
    dew = dewhiten(raw, reset_index=0)
    swapped = swap_pairs(dew)
    bits, metric, end_state = viterbi_decode(swapped, 54)
    hdr = parse_header(bits)
    crc_ok = crc16_bitserial(bits[:38]) == hdr['crc']
    return hdr, bits, metric, crc_ok


if __name__ == '__main__':
    import sys
    path = sys.argv[1] if len(sys.argv) > 1 else 'sampleaudio/wav/KG-STV_MSK_text.wav'
    sr, data = load_mono(path)
    inst = fsk_discriminator(data, sr)
    clk = BitClock(inst, sr)
    anchor, sync_pos = find_anchor_and_sync(clk)
    print("anchor:", anchor, "sync at bit:", sync_pos)
    info_start = sync_pos + 63
    hdr, bits, metric, crc_ok = decode_header_at(clk, anchor, info_start)
    print("header:", hdr, "viterbi_metric:", metric, "crc_ok:", crc_ok)
