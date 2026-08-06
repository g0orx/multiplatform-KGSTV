#!/usr/bin/env python3
"""
Bir KG-STV WAV kaydını (örn. WebSDR'ın kendi kayıt özelliğiyle indirilmiş) baştan sona
decode edip, çözülebilen görüntü bloklarından 320x240 bir PNG üretir.

Kullanım:
    python3 decode_session.py <wav_dosyasi.wav> [cikti.png]

Notlar:
- MSK modundaki (m=0) görüntü blokları tam olarak decode edilir (framing + JPEG).
- 4L-FSK (m=1) blokları şu an güvenilir demodüle edilemiyor (bkz. docs/protocol_spec.md,
  "4L-FSK" bölümü) — bu bloklar siyah (0,0,0) bırakılır, session ilerlemeye devam eder.
- Eksik/hatalı bloklar siyah kalır — bu KG-STV'nin kendi görsel davranışıyla aynı.
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
    """Uzun (dakikalar süren) gerçek kayıtlarda KG-STV yayınının NEREDE başladığı bilinmez.
    256-bit '01' preamble'ı, 1200 baud'da tam 600Hz'de alternatif bir sinyal üretir; bunu
    ani-frekans (discriminator) çıkışında kayan bir Goertzel/FFT penceresiyle arayıp güçlü
    600Hz enerjisi olan bölgeleri (aday preamble konumları) hızlıca buluyoruz."""
    from kgstv_decode import fsk_discriminator
    from scipy.signal import stft
    inst = fsk_discriminator(samples, sr)
    win = max(64, int(sr * 0.05))  # ~50ms pencere
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
    """Kaydın tamamını tarayarak preamble/sync'i bulur: önce spektral aday konumlar,
    sonra her adayın etrafında hassas arama."""
    candidates, _ = find_preamble_candidates(samples, sr)
    if verbose:
        print(f"aday preamble konumları (örnek): {candidates}")
    best_overall = None
    for c in candidates:
        lo = max(0, c - int(sr * 0.15))
        hi = c + int(sr * 0.15)
        step = max(1, int(sr / 8000))
        anchor, err = find_first_sync(clk, range(lo, hi, step), fine_span=step * 3, fine_step=0.05)
        if verbose:
            print(f"  aday @{c} ({c/sr:.1f}s) -> anchor={anchor:.1f} err={err}")
        if best_overall is None or err < best_overall[1]:
            best_overall = (anchor, err)
        if err == 0:
            break
    return best_overall


class SyncLocator:
    """Uzun/gürültülü gerçek kayıtlarda 63-bit senkron kelimesini vektörize korelasyonla hızlıca
    bulan yardımcı. Tüm dosya için ani-frekans dizisini ve kümülatif toplamını bir kez hesaplar;
    ardından herhangi bir bit-aralığında (birden çok faz denenerek) en iyi eşleşmeyi arar."""

    def __init__(self, samples, sr):
        from kgstv_decode import fsk_discriminator
        self.sr = sr
        self.Tb = sr / 1200.0
        self.inst = fsk_discriminator(samples, sr)
        self.cum = np.cumsum(self.inst)
        self.sync_pm1 = np.array([1 if c == '1' else -1 for c in SYNC_WORD], dtype=np.float64)
        self.idx_arr = np.arange(len(self.cum))

    def search(self, start_symbol, n_symbols, n_phases=24):
        """[start_symbol, start_symbol+n_symbols) sembol aralığında (faz=0 referansıyla)
        en iyi senkron eşleşmesini arar. Dönüş: (matches, symbol_idx, phase)."""
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
    """pos'tan hemen önceki 63 bit'in gerçekten senkron kelimesi olup olmadığını kontrol edip,
    küçük bir zamanlama kaymasını (drift) düzeltmek için anchor'ı yerel olarak ince ayarlar.
    Uzun kayıtlarda (>1 dakika) verici/alıcı saat kayması birikir; bu, her frame'de küçük bir
    düzeltme yaparak takip eder (basit bir zamanlama kurtarma/tracking döngüsü)."""
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
    """resync_local sadece senkron kelimesinin bit-hatasını minimize eder; bu her zaman
    header'ın Viterbi metriğini minimize eden anchor ile birebir örtüşmez (özellikle gerçek,
    gürültülü kayıtlarda). Burada anchor_base'den başlayıp bir tam sembol periyodu (tb) boyunca
    tarayarak header'ın kendi Viterbi metriğini DOĞRUDAN minimize eden anchor'ı buluyoruz —
    108 bitlik header için ucuz (bir frame başına ~tb/step deneme), ve gürültülü gerçek
    kayıtlarda çok daha güvenilir."""
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
    """y_blocks: [Y0,Y1,Y2,Y3] (sol-üst, sağ-üst, sol-alt, sağ-alt), her biri 8x8.
    cb, cr: 8x8 (16x16 tile'ın tamamını kapsıyor, 2x upsample gerekiyor)."""
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
    """pos'taki header'ı (ve varsa veri chunk'ını) bağımsız olarak decode etmeyi dener.
    Sıralı bir 'walk' varsayımı YAPMAZ — her aday kendi başına, header'ın kendi x/y/size
    alanlarına güvenerek decode edilir. Gürültülü/parçalı gerçek kayıtlarda bu, bir frame'in
    başarısız olmasının sonraki tüm frame'leri de düşürmesini önler.
    Dönüş: None (header çözülemedi) ya da bir sonuç sözlüğü."""
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
        result['note'] = "4L-FSK — demod kalibre değil"
        return result
    data_bits, dcrc_ok = decode_data_bits(clk, anchor, data_start, hdr['size'], conv)
    result['data_bits'] = data_bits
    result['data_crc_ok'] = dcrc_ok
    return result


def place_image_tile(image, got_block, hdr, data_bits):
    """decode_16x16_block çıktısını YCbCr->RGB'ye çevirip görüntüye yerleştirir.
    Başarıyla yerleştirildiyse True döner."""
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
    """Uzun/gürültülü gerçek kayıtlar için: dosyanın tamamındaki güçlü senkron adaylarını
    (>=55/63 eşleşme) bulur ve HER BİRİNİ BAĞIMSIZ olarak decode etmeyi dener — sıra veya
    süreklilik varsaymaz. Her başarılı IMAGE/BSR frame'i kendi x,y koordinatına yerleştirilir."""
    Tb = sr / 1200.0
    locator = SyncLocator(data, sr)
    n_symbols = int((len(locator.cum) - Tb) / Tb)
    print(f"dosyanın tamamı taranıyor ({n_symbols} sembol, birkaç dakika sürebilir)...")

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
    print(f"{len(all_candidates)} güçlü senkron adayı bulundu (dedup öncesi)")

    image = np.zeros((IMG_H, IMG_W, 3), dtype=np.uint8)
    got_block = np.zeros((IMG_H // TILE, IMG_W // TILE), dtype=bool)
    seen_bins = set()
    n_header_ok = n_image = n_crc_ok = n_decoded = n_fsk4_skipped = 0
    texts = []
    best_tile = {}  # (x,y) -> (metric, crc_ok) — bir karo birden fazla kez görülürse en iyisini tut

    for matches, sidx, phase in all_candidates:
        key = sidx // 15  # yakın adayları tekilleştir
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
                # NOT: 'best-effort' (CRC hatalı da olsa yerleştir) denendi, ama NORM (c=0)
                # veride tek bit hatası JPEG Huffman akışını tamamen bozuyor (gürültü çıkıyor,
                # görüntü değil) — bu yüzden sadece CRC-geçerli bloklar yerleştiriliyor.
                # Bu, gerçek kanalda BSR olmadan pasif dinlemenin doğal sınırıdır.
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

    print(f"\n--- Özet ---")
    print(f"{len(all_candidates)} senkron adayından {n_header_ok} header çözüldü")
    print(f"{n_image} görüntü frame'i, {n_crc_ok} CRC-geçerli, {n_decoded} piksele çözüldü")
    if n_fsk4_skipped:
        print(f"{n_fsk4_skipped} blok 4L-FSK olduğu için atlandı")
    print(f"doldurulan karo: {got_block.sum()}/{got_block.size}")
    if texts:
        print("Okunan metinler:")
        for t, cmd, txt in texts:
            print(f"  t={t:.1f}s [{cmd}] {txt!r}")

    save_image(image, out_path)


def save_image(image, out_path):
    try:
        from PIL import Image
        Image.fromarray(image).save(out_path)
        print("kaydedildi:", out_path)
    except ImportError:
        np.save(out_path + ".npy", image)
        print("Pillow bulunamadı, ham array kaydedildi:", out_path + ".npy")
        print("Kurulum için: pip install pillow")


def decode_short_recording(clk, bp, sr, out_path):
    """Temiz/kısa kayıtlar için (bkz. sampleaudio testleri): tek bir senkron bulunduktan
    sonra sıralı olarak (bir frame'in sonu = sonrakinin başı) yürür — sürekliliğe güvenir."""
    Tb = sr / 1200.0
    search_range = range(0, min(8000, max(1, len(bp) - 400)), 2)
    anchor, err = find_first_sync(clk, search_range)
    pos = PREAMBLE_LEN + SYNC_LEN
    print(f"anchor={anchor:.2f}  pos={pos}  preamble+sync hata={err}/63")

    image = np.zeros((IMG_H, IMG_W, 3), dtype=np.uint8)
    got_block = np.zeros((IMG_H // TILE, IMG_W // TILE), dtype=bool)
    n_image = n_crc_ok = n_decoded = n_fsk4_skipped = 0
    consecutive_fail = 0

    for i in range(3000):
        if pos + HEADER_ENC_LEN > len(bp):
            print(f"frame{i}@{pos}: ses bitti, duruluyor.")
            break
        anchor2, metric, hdr, crc_ok = find_best_header_anchor(clk, anchor, pos, Tb)
        if crc_ok:
            anchor = anchor2
        if not crc_ok:
            consecutive_fail += 1
            print(f"frame{i}@{pos}: header CRC hatası (metric={metric}) — "
                  f"{'duruluyor' if consecutive_fail >= 5 else 'atlanıp deneniyor'}")
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
                note = "4L-FSK — demod kalibre değil, atlanıyor"
                data_bits, dcrc_ok = [], False
            else:
                data_bits, dcrc_ok = decode_data_bits(clk, anchor, data_start, hdr['size'], conv)
                if dcrc_ok:
                    n_crc_ok += 1
                    if place_image_tile(image, got_block, hdr, data_bits):
                        n_decoded += 1
                        note = "OK"
                    else:
                        note = "eksik JPEG blok"
            print(f"frame{i}@{pos}: {cmd} tile=({hdr['x']:2d},{hdr['y']:2d}) sc={hdr['sc']:2d} "
                  f"size={hdr['size']:4d}b crc_ok={dcrc_ok}  {note}")
        elif hdr['com'] in (0, 6):
            data_bits, dcrc_ok = decode_data_bits(clk, anchor, data_start, hdr['size'], conv)
            print(f"frame{i}@{pos}: {cmd} crc_ok={dcrc_ok} metin={bits_to_ascii(data_bits)!r}")

        enc_len = (hdr['size'] + 16) * (2 if conv else 1)
        pos = data_start + enc_len + SYNC_LEN

    print(f"\n--- Özet ---")
    print(f"{n_image} görüntü frame'i görüldü, {n_crc_ok} CRC-geçerli, {n_decoded} piksele çözüldü")
    if n_fsk4_skipped:
        print(f"{n_fsk4_skipped} blok 4L-FSK olduğu için atlandı (bkz. docs/protocol_spec.md)")
    print(f"doldurulan karo: {got_block.sum()}/{got_block.size}")
    save_image(image, out_path)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    wav_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) > 2 else "decoded_image.png"

    sr, data = load_mono(wav_path)
    print(f"yüklendi: {wav_path}  sr={sr:.0f}Hz  süre={len(data) / sr:.1f}s")

    bp = bandpass(data, sr)
    clk = ToneClock(bp, sr)

    if len(data) / sr > 5.0:
        decode_long_recording(clk, data, sr, out_path)
    else:
        decode_short_recording(clk, bp, sr, out_path)


if __name__ == "__main__":
    main()
