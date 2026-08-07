"""
Standart JPEG (ITU T.81 Annex K) Huffman tabloları + zigzag + IDCT ile,
KG-STV'nin ham (marker'sız, byte-hizasız) 16x16 blok entropy-bitstream'ini
piksellere çevirmeyi deneyen minimal bir decoder.

Bu bir DENEY: KG-STV'nin gerçekten standart tabloları kullanıp kullanmadığını
test ediyoruz.
"""
import numpy as np

ZIGZAG = [
    0,1,8,16,9,2,3,10,
    17,24,32,25,18,11,4,5,
    12,19,26,33,40,48,41,34,
    27,20,13,6,7,14,21,28,
    35,42,49,56,57,50,43,36,
    29,22,15,23,30,37,44,51,
    58,59,52,45,38,31,39,46,
    53,60,61,54,47,55,62,63,
]

STD_LUMA_QT = np.array([
    16,11,10,16,24,40,51,61,
    12,12,14,19,26,58,60,55,
    14,13,16,24,40,57,69,56,
    14,17,22,29,51,87,80,62,
    18,22,37,56,68,109,103,77,
    24,35,55,64,81,104,113,92,
    49,64,78,87,103,121,120,101,
    72,92,95,98,112,100,103,99,
]).reshape(8,8)

STD_CHROMA_QT = np.array([
    17,18,24,47,99,99,99,99,
    18,21,26,66,99,99,99,99,
    24,26,56,99,99,99,99,99,
    47,66,99,99,99,99,99,99,
    99,99,99,99,99,99,99,99,
    99,99,99,99,99,99,99,99,
    99,99,99,99,99,99,99,99,
    99,99,99,99,99,99,99,99,
]).reshape(8,8)

def scale_qt(qt, quality):
    quality = max(1, min(100, quality))
    scale = 5000/quality if quality < 50 else 200 - 2*quality
    out = np.floor((qt*scale + 50)/100)
    return np.clip(out, 1, 255)

DC_LUMA = (
    [0,1,5,1,1,1,1,1,1,0,0,0,0,0,0,0],
    [0,1,2,3,4,5,6,7,8,9,10,11]
)
DC_CHROMA = (
    [0,3,1,1,1,1,1,1,1,1,1,0,0,0,0,0],
    [0,1,2,3,4,5,6,7,8,9,10,11]
)
AC_LUMA = (
    [0,2,1,3,3,2,4,3,5,5,4,4,0,0,1,0x7d],
    [0x01,0x02,0x03,0x00,0x04,0x11,0x05,0x12,
     0x21,0x31,0x41,0x06,0x13,0x51,0x61,0x07,
     0x22,0x71,0x14,0x32,0x81,0x91,0xa1,0x08,
     0x23,0x42,0xb1,0xc1,0x15,0x52,0xd1,0xf0,
     0x24,0x33,0x62,0x72,0x82,0x09,0x0a,0x16,
     0x17,0x18,0x19,0x1a,0x25,0x26,0x27,0x28,
     0x29,0x2a,0x34,0x35,0x36,0x37,0x38,0x39,
     0x3a,0x43,0x44,0x45,0x46,0x47,0x48,0x49,
     0x4a,0x53,0x54,0x55,0x56,0x57,0x58,0x59,
     0x5a,0x63,0x64,0x65,0x66,0x67,0x68,0x69,
     0x6a,0x73,0x74,0x75,0x76,0x77,0x78,0x79,
     0x7a,0x83,0x84,0x85,0x86,0x87,0x88,0x89,
     0x8a,0x92,0x93,0x94,0x95,0x96,0x97,0x98,
     0x99,0x9a,0xa2,0xa3,0xa4,0xa5,0xa6,0xa7,
     0xa8,0xa9,0xaa,0xb2,0xb3,0xb4,0xb5,0xb6,
     0xb7,0xb8,0xb9,0xba,0xc2,0xc3,0xc4,0xc5,
     0xc6,0xc7,0xc8,0xc9,0xca,0xd2,0xd3,0xd4,
     0xd5,0xd6,0xd7,0xd8,0xd9,0xda,0xe1,0xe2,
     0xe3,0xe4,0xe5,0xe6,0xe7,0xe8,0xe9,0xea,
     0xf1,0xf2,0xf3,0xf4,0xf5,0xf6,0xf7,0xf8,
     0xf9,0xfa]
)
AC_CHROMA = (
    [0,2,1,2,4,4,3,4,7,5,4,4,0,1,2,0x77],
    [0x00,0x01,0x02,0x03,0x11,0x04,0x05,0x21,
     0x31,0x06,0x12,0x41,0x51,0x07,0x61,0x71,
     0x13,0x22,0x32,0x81,0x08,0x14,0x42,0x91,
     0xa1,0xb1,0xc1,0x09,0x23,0x33,0x52,0xf0,
     0x15,0x62,0x72,0xd1,0x0a,0x16,0x24,0x34,
     0xe1,0x25,0xf1,0x17,0x18,0x19,0x1a,0x26,
     0x27,0x28,0x29,0x2a,0x35,0x36,0x37,0x38,
     0x39,0x3a,0x43,0x44,0x45,0x46,0x47,0x48,
     0x49,0x4a,0x53,0x54,0x55,0x56,0x57,0x58,
     0x59,0x5a,0x63,0x64,0x65,0x66,0x67,0x68,
     0x69,0x6a,0x73,0x74,0x75,0x76,0x77,0x78,
     0x79,0x7a,0x82,0x83,0x84,0x85,0x86,0x87,
     0x88,0x89,0x8a,0x92,0x93,0x94,0x95,0x96,
     0x97,0x98,0x99,0x9a,0xa2,0xa3,0xa4,0xa5,
     0xa6,0xa7,0xa8,0xa9,0xaa,0xb2,0xb3,0xb4,
     0xb5,0xb6,0xb7,0xb8,0xb9,0xba,0xc2,0xc3,
     0xc4,0xc5,0xc6,0xc7,0xc8,0xc9,0xca,0xd2,
     0xd3,0xd4,0xd5,0xd6,0xd7,0xd8,0xd9,0xda,
     0xe2,0xe3,0xe4,0xe5,0xe6,0xe7,0xe8,0xe9,
     0xea,0xf2,0xf3,0xf4,0xf5,0xf6,0xf7,0xf8,
     0xf9,0xfa]
)


def build_huffman_decode_table(counts, values):
    """Returns dict: (length,code)->value, via canonical JPEG code assignment (Annex C)."""
    codes = {}
    code = 0
    k = 0
    for length in range(1, 17):
        for _ in range(counts[length-1]):
            codes[(length, code)] = values[k]
            k += 1
            code += 1
        code <<= 1
    return codes


class BitReader:
    def __init__(self, bits):
        self.bits = bits
        self.pos = 0
    def read_bit(self):
        if self.pos >= len(self.bits):
            raise EOFError
        b = self.bits[self.pos]
        self.pos += 1
        return b
    def read_bits(self, n):
        v = 0
        for _ in range(n):
            v = (v<<1) | self.read_bit()
        return v
    def remaining(self):
        return len(self.bits) - self.pos


def decode_huffman_symbol(br, table):
    code = 0
    for length in range(1, 17):
        code = (code << 1) | br.read_bit()
        if (length, code) in table:
            return table[(length, code)]
    raise ValueError("no matching huffman code")


def extend(v, s):
    if s == 0:
        return 0
    vt = 1 << (s-1)
    return v if v >= vt else v - (1 << s) + 1


def decode_block(br, dc_table, ac_table, dc_pred):
    coeffs = [0]*64
    s = decode_huffman_symbol(br, dc_table)
    diff = extend(br.read_bits(s), s) if s else 0
    dc = dc_pred + diff
    coeffs[0] = dc
    k = 1
    while k < 64:
        rs = decode_huffman_symbol(br, ac_table)
        r, sz = rs >> 4, rs & 0xF
        if rs == 0x00:  # EOB
            break
        if rs == 0xF0:  # ZRL
            k += 16
            continue
        k += r
        if k >= 64:
            break
        val = extend(br.read_bits(sz), sz) if sz else 0
        coeffs[k] = val
        k += 1
    return coeffs, dc


def idct8(block):
    # naive O(n^4) IDCT via basis matrix, fine for exploration
    x = np.arange(8)
    C = np.cos((2*x[:,None]+1)*x[None,:]*np.pi/16)
    alpha = np.array([np.sqrt(1/8)] + [np.sqrt(2/8)]*7)
    T = (C * alpha[None,:])
    return T @ block @ T.T


def decode_16x16_block(bits, quality=10, verbose=False):
    """bits: list of 0/1, MSB-first, for one 16x16 KG-STV image block (4 Y + 1 Cb + 1 Cr)."""
    br = BitReader(bits)
    dc_luma_tbl = build_huffman_decode_table(*DC_LUMA)
    dc_chroma_tbl = build_huffman_decode_table(*DC_CHROMA)
    ac_luma_tbl = build_huffman_decode_table(*AC_LUMA)
    ac_chroma_tbl = build_huffman_decode_table(*AC_CHROMA)
    qt_y = scale_qt(STD_LUMA_QT, quality)
    qt_c = scale_qt(STD_CHROMA_QT, quality)

    results = []
    # Gerçek off-air kayıtla (kontrollü düz-renk test görüntüleriyle: tamamen düz kırmızı/mavi
    # bir 16x16 karonun 4 Y bloğu arasında DC "farkının" MUTLAKA sıfıra yakın olması gerekirken,
    # bizim eski kodumuz her blokta AYNI, sabit bir "diff" değerini BİRİKTİREREK (chain ederek)
    # her seferinde daha da büyüyen, saçma bir DC dizisi üretiyordu — örn. düz kırmızıda
    # -58,-116,-174,-232 (aslında her blok bağımsız olarak DC=-58 taşıyor olmalıydı).
    # Sonuç: KG-STV, standart JPEG'in aksine, bir tile içindeki 6 bloğun DC değerlerini
    # BİRBİRİNE ZİNCİRLEMEDEN, her birini kendi başına (predictor=0) kodluyor.
    plan = [('Y',0),('Y',0),('Y',0),('Y',0),('Cr',0),('Cb',0)]
    try:
        for comp, _ in plan:
            if comp == 'Y':
                coeffs, _ = decode_block(br, dc_luma_tbl, ac_luma_tbl, 0)
                qt = qt_y
            elif comp == 'Cr':
                coeffs, _ = decode_block(br, dc_chroma_tbl, ac_chroma_tbl, 0)
                qt = qt_c
            else:
                coeffs, _ = decode_block(br, dc_chroma_tbl, ac_chroma_tbl, 0)
                qt = qt_c
            block = np.zeros((8,8))
            for i,c in enumerate(coeffs):
                r,cc = divmod(ZIGZAG[i], 8)
                block[r,cc] = c
            deq = block * qt
            px = np.clip(idct8(deq) + 128, 0, 255)
            results.append((comp, px))
            if verbose:
                print(comp, "DC=", coeffs[0], "nonzero_ac=", sum(1 for c in coeffs[1:] if c))
    except (EOFError, ValueError) as e:
        if verbose:
            print("decode stopped:", e, "bits consumed:", br.pos, "/", len(bits))
    return results, br.pos
