"""
REAL FIR filter coefficients extracted from kgstv.exe (via radare2 disassembly).

Function @0x4033e8 (waveIn callback), via radare2: the audio input callback applies this chain:
  1. int16 PCM samples are converted to double (44100 Hz)
  2. filtered with FILTER1 (51-tap, anti-alias lowpass)
  3. divided by 4 (decimate) -> 11025 Hz
  4. filtered with FILTER2 (51-tap, ~900-2000Hz bandpass, selects the KG-STV tone band)
  5. (the rest not fully extracted yet — symbol timing/decision logic)

Location: kgstv.exe file offset 841240 (FILTER1) and 841648 (FILTER2),
each 51 x 8-byte little-endian doubles, consecutive.
"""

# 51-tap, centre coefficient 0.5 — anti-alias / pre-decimation lowpass (runs at 44100Hz)
FILTER1 = [
    0.00188395336, -3.57324275e-18, -0.00305968932, 1.85021074e-17, 0.00458956204,
    -6.74764434e-18, -0.00654922183, 8.50952068e-18, 0.0090416337, -1.03180314e-17,
    -0.0122175793, 1.21136044e-17, 0.0163156089, -1.38340504e-17, -0.0217460045,
    1.54174615e-17, 0.029286942, -1.68051683e-17, -0.0406168791, 1.79445932e-17,
    0.0601240729, -1.8791841e-17, -0.103952335, 1.93138838e-17, 0.317588148, 0.5,
    0.317588148, 1.93138838e-17, -0.103952335, -1.8791841e-17, 0.0601240729,
    1.79445932e-17, -0.0406168791, -1.68051683e-17, 0.029286942, 1.54174615e-17,
    -0.0217460045, -1.38340504e-17, 0.0163156089, 1.21136044e-17, -0.0122175793,
    -1.03180314e-17, 0.0090416337, 8.50952068e-18, -0.00654922183, -6.74764434e-18,
    0.00458956204, 1.85021074e-17, -0.00305968932, -3.57324275e-18, 0.00188395336,
]

# 51-tap, centre coefficient 1/3 — channel-selecting bandpass (~900-2000Hz @ 11025Hz), after decimation
FILTER2 = [
    0.000183556493, -4.58719924e-19, -0.000530567958, 4.75300479e-18, 0.00225312673,
    0.00378769117, 0.00206474048, 1.54424232e-33, 0.00345920986, 0.0107074434,
    0.0109110324, -7.07502541e-18, -0.00825482848, 6.95939328e-18, 0.0121696272,
    -1.1026314e-17, -0.0355191016, -0.0526781891, -0.0262061786, -2.0552463e-33,
    -0.0405923148, -0.129031334, -0.144599161, 1.66040582e-17, 0.224157762,
    0.333333333, 0.224157762, 1.66040582e-17, -0.144599161, -0.129031334,
    -0.0405923148, -2.0552463e-33, -0.0262061786, -0.0526781891, -0.0355191016,
    -1.1026314e-17, 0.0121696272, 6.95939328e-18, -0.00825482848, -7.07502541e-18,
    0.0109110324, 0.0107074434, 0.00345920986, 1.54424232e-33, 0.00206474048,
    0.00378769117, 0.00225312673, 4.75300479e-18, -0.000530567958, -4.58719924e-19,
    0.000183556493,
]

DECIMATION_FACTOR = 4
NATIVE_SAMPLE_RATE = 44100
DECIMATED_SAMPLE_RATE = NATIVE_SAMPLE_RATE / DECIMATION_FACTOR  # 11025


# 51-tap, centre coefficient 0.0, fully antisymmetric — Hilbert transformer (produces the quadrature/imag component)
FILTER3 = [
    -0.00589720752, -4.0817023e-19, -0.00592987191, 3.01571897e-16, -0.00882375805,
    2.61318234e-16, -0.0125953651, 3.95410658e-16, -0.017460947, 5.61850573e-17,
    -0.0237307016, 1.41339571e-16, -0.0318879867, 1.62852481e-16, -0.042756742,
    7.62289792e-17, -0.0578935177, -2.90024081e-17, -0.0806522041, 1.38098657e-16,
    -0.119803225, 5.83874758e-17, -0.207625476, -6.45985193e-17, -0.635081148, 0.0,
    0.635081148, 6.45985193e-17, 0.207625476, -5.83874758e-17, 0.119803225,
    -1.38098657e-16, 0.0806522041, 2.90024081e-17, 0.0578935177, -7.62289792e-17,
    0.042756742, -1.62852481e-16, 0.0318879867, -1.41339571e-16, 0.0237307016,
    -5.61850573e-17, 0.017460947, -3.95410658e-16, 0.0125953651, -2.61318234e-16,
    0.00882375805, -3.01571897e-16, 0.00592987191, 4.0817023e-19, 0.00589720752,
]

# 51-tap, centre coefficient 0.125, symmetric lowpass — final smoothing of the instantaneous frequency estimate
FILTER4 = [
    -9.9340058e-05, 1.72019972e-19, 0.00028714131, 0.000792037935, 0.00147192597,
    0.00218682451, 0.00269771345, 0.00269676853, 0.00187211088, -1.51408991e-18,
    -0.0029525041, -0.0067388816, -0.0107854532, -0.0142074089, -0.0159003842,
    -0.014703415, -0.00961139964, 3.72448919e-18, 0.0141826817, 0.0323023863,
    0.0530364151, 0.0744962753, 0.0944639542, 0.110706242, 0.121313306, 0.125,
    0.121313306, 0.110706242, 0.0944639542, 0.0744962753, 0.0530364151,
    0.0323023863, 0.0141826817, 3.72448919e-18, -0.00961139964, -0.014703415,
    -0.0159003842, -0.0142074089, -0.0107854532, -0.0067388816, -0.0029525041,
    -1.51408991e-18, 0.00187211088, 0.00269676853, 0.00269771345, 0.00218682451,
    0.00147192597, 0.000792037935, 0.00028714131, 1.72019972e-19, -9.9340058e-05,
]

# Phase unwrap constant — found exactly in the assembly: 0x403bc4 = 2π
TWO_PI = 6.283185307179586


def apply_authentic_discriminator(samples_44100):
    """Applies the full RX chain (4 filters + Hilbert + phase difference).
    Input: 44100Hz mono float array. Output: (instantaneous frequency array at 11025Hz, Hz).
    NOTE: This implementation reflects the architecture correctly (from the kgstv.exe disassembly),
    but the Hilbert sign convention / phase direction does not bit-match the assembly exactly —
    the simple complex-mixing approach (kgstv_decode.fsk_discriminator) is still more reliable.
    See docs/protocol_spec.md, '4L-FSK' section.
    """
    from scipy.signal import lfilter
    import numpy as np
    f1 = lfilter(FILTER1, [1.0], samples_44100)
    dec = f1[::DECIMATION_FACTOR]
    real = lfilter(FILTER2, [1.0], dec)
    imag = lfilter(FILTER3, [1.0], real)
    phase = np.arctan2(imag, real)
    unwrapped = np.unwrap(phase)
    freq = np.diff(unwrapped) * DECIMATED_SAMPLE_RATE / TWO_PI
    return lfilter(FILTER4, [1.0], freq)


def apply_authentic_frontend(samples_44100):
    """Applies kgstv.exe's real front-end filter chain. Input: 44100Hz mono float array.
    Output: (signal decimated to 11025Hz, bandpass filtered)."""
    from scipy.signal import lfilter
    f1 = lfilter(FILTER1, [1.0], samples_44100)
    dec = f1[::DECIMATION_FACTOR]
    f2 = lfilter(FILTER2, [1.0], dec)
    return f2
