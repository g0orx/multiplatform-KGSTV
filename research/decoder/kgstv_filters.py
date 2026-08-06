"""
kgstv.exe'den (radare2 disassembly ile) çıkarılan GERÇEK FIR filtre katsayıları.

Fonksiyon @0x4033e8 (waveIn callback), radare2 ile: ses girişi callback'i şu zinciri uygular:
  1. int16 PCM örnekleri double'a çevrilir (44100 Hz)
  2. FILTER1 (51-tap, anti-alias lowpass) ile filtrelenir
  3. 4'e böler (decimate) -> 11025 Hz
  4. FILTER2 (51-tap, ~900-2000Hz bandpass, KG-STV ton bandını seçer) ile filtrelenir
  5. (ötesi henüz tam çıkarılmadı — sembol zamanlama/karar mantığı)

Bulunma yeri: kgstv.exe dosya offset 841240 (FILTER1) ve 841648 (FILTER2),
her biri 51 x 8-byte little-endian double, ardışık.
"""

# 51-tap, merkez katsayı 0.5 — anti-alias / decimasyon öncesi lowpass (44100Hz'de çalışıyor)
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

# 51-tap, merkez katsayı 1/3 — kanal-seçici bandpass (~900-2000Hz @ 11025Hz), decimasyon sonrası
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


# 51-tap, merkez katsayı 0.0, tam antisimetrik — Hilbert dönüştürücü (quadrature/imag bileşen üretir)
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

# 51-tap, merkez katsayı 0.125, simetrik lowpass — anlık frekans kestiriminin son yumuşatması
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

# Faz açma (unwrap) sabiti — assembly'de tam olarak bulundu: 0x403bc4 = 2π
TWO_PI = 6.283185307179586


def apply_authentic_discriminator(samples_44100):
    """Tam RX zincirini (4 filtre + Hilbert + faz-farkı) uygular.
    Girdi: 44100Hz mono float dizisi. Çıktı: (11025Hz'de anlık frekans dizisi, Hz).
    NOT: Bu implementasyon mimariyi doğru şekilde yansıtıyor (kgstv.exe disassembly'sinden),
    ancak Hilbert işaret kuralı / faz yönü tam olarak assembly ile bit-eşleşmiyor —
    basit karmaşık-mixing yaklaşımı (kgstv_decode.fsk_discriminator) hâlâ daha güvenilir.
    Bkz. docs/protocol_spec.md, '4L-FSK' bölümü.
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
    """kgstv.exe'nin gerçek ön-uç filtre zincirini uygular. Girdi: 44100Hz mono float dizisi.
    Çıktı: (11025Hz'e decimate edilmiş, bandpass filtrelenmiş sinyal)."""
    from scipy.signal import lfilter
    f1 = lfilter(FILTER1, [1.0], samples_44100)
    dec = f1[::DECIMATION_FACTOR]
    f2 = lfilter(FILTER2, [1.0], dec)
    return f2
