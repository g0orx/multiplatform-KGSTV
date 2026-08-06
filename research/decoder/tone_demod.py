"""Robust tone-correlation FSK/MSK demodulator (non-coherent, per-symbol DFT correlation).
More robust than the instantaneous-frequency discriminator for noisier/longer captures.
"""
import numpy as np
from scipy import signal

SR_ASSUMED = 44100
BAUD = 1200.0

MSK_FREQS = [1200.0, 1800.0]           # bit 0, bit 1
FSK4_FREQS = [1200.0, 1400.0, 1600.0, 1800.0]  # level 0..3 -> bits per KG-STV table


def bandpass(data, sr, lo=900, hi=2200):
    b, a = signal.butter(4, [lo/(sr/2), hi/(sr/2)], btype='band')
    return signal.filtfilt(b, a, data)


class ToneClock:
    def __init__(self, data, sr, baud=BAUD):
        self.data = data
        self.sr = sr
        self.Tb = sr / baud

    def symbol_window(self, t0, i):
        a = int(round(t0 + i * self.Tb))
        b = int(round(t0 + (i + 1) * self.Tb))
        if b <= a:
            b = a + 1
        return self.data[a:b]

    def tone_powers(self, t0, i, freqs):
        seg = self.symbol_window(t0, i)
        n = len(seg)
        if n == 0:
            return [0.0] * len(freqs)
        t = np.arange(n)
        win = np.hanning(n) if n > 3 else np.ones(n)
        powers = []
        for f in freqs:
            ref = np.exp(-1j * 2 * np.pi * f * t / self.sr)
            corr = np.sum(seg * win * ref)
            powers.append(abs(corr))
        return powers

    def msk_bit(self, t0, i):
        p = self.tone_powers(t0, i, MSK_FREQS)
        return 0 if p[0] > p[1] else 1

    def msk_bits(self, t0, n):
        return [self.msk_bit(t0, i) for i in range(n)]

    def fsk4_symbol(self, t0, i):
        p = self.tone_powers(t0, i, FSK4_FREQS)
        return int(np.argmax(p))  # 0..3 -> maps directly to 00,01,10,11

    def fsk4_bits(self, t0, n_symbols):
        bits = []
        for i in range(n_symbols):
            lvl = self.fsk4_symbol(t0, i)
            bits += [(lvl >> 1) & 1, lvl & 1]
        return bits


def find_anchor_msk(clk, coarse_range, preamble_len=256):
    alt0 = [i % 2 for i in range(preamble_len)]
    best = None
    for t0 in coarse_range:
        bits = clk.msk_bits(t0, preamble_len)
        err = sum(a != b for a, b in zip(bits, alt0))
        if best is None or err < best[0]:
            best = (err, t0)
    return best
