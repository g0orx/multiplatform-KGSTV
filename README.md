# KGSTV — Cross-Platform Yeniden Yazım

**Amaç:** JJ0OBZ (K.G., Niigata-Myoko, Japonya) tarafından yazılan, sadece Windows'ta çalışan
**KG-STV** (dijital SSTV) yazılımının; Windows, Linux, macOS ve (mümkünse) iOS/Android'de
çalışan, havadaki orijinal KG-STV istasyonlarıyla uyumlu bir yeniden yazımını yapmak.

## Durum

Protokol, orijinal `readme.txt` çevirisi + gerçek off-air ses kayıtları (4 farklı istasyon:
TA4SO, TA2VB, DL4CQ + kontrollü OH2UDS test kayıtları) üzerinde bit-seviyesinde ters
mühendislikle **doğrulandı**. Ayrıntılar: [`docs/protocol_spec.md`](docs/protocol_spec.md).

| Katman | Durum |
|---|---|
| Framing (preamble/sync/frame-walk) | ✅ Gerçek kayıtla doğrulandı |
| Konvolüsyonel kod (K=7, 171/133 oktal) | ✅ Doğrulandı, Rust'ta implement edildi |
| Whitening (127-bit M-sequence) | ✅ Doğrulandı, Rust'ta implement edildi |
| CRC (CRC-16/AUG-CCITT) | ✅ Doğrulandı, Rust'ta implement edildi |
| MSK demodülasyon | ✅ Üretim kalitesinde (Python), gerçek istasyonlarla doğrulandı |
| JPEG Huffman/entropy tabloları | ✅ Standart tablolar bit-uyumlu doğrulandı |
| JPEG kuantizasyon ölçeği (`sc`→scale) | ✅ Çözüldü — `sc` doğrudan IJG quality (1-100) |
| JPEG blok içi DC kodlama | ✅ Kritik hata bulundu ve düzeltildi (6 blok birbirine zincirlenmez) |
| **Rust JPEG codec** (`kgstv-core::jpeg`) | ✅ Encode+decode, gerçek kayıt verisiyle doğrulandı |
| 4L-FSK demodülasyon kalibrasyonu | ⚠️ Kısmi (~%80-83 sembol doğruluğu, yetersiz) |
| Rust ses I/O + modülatör/demodülatör | ⏳ Henüz başlanmadı |
| Platform UI'ları | ⏳ Henüz başlanmadı |

## Dizin Yapısı

- `docs/` — araştırma ve protokol dokümantasyonu
  - `research.md` — web araştırması bulguları
  - `protocol_spec.md` — **otoriter kaynak**: tam protokol spesifikasyonu + doğrulama kayıtları
  - `HISTORY.md`, `OVERVIEW_EN.md`, `kgstv-protocol-reference.html` — İngilizce tarihçe/teknik özet
- `research/` — ters mühendislik çalışma alanı
  - `binaries/` — orijinal `kgstv.zip` (KGSTV.exe, readme'ler, kgstv.ini)
  - `sampleaudio/` — sentetik test kayıtları (MSK/4LFSK, Norm/Convolution, farklı JPEG scale)
  - `decoder/` — Python prototip decoder:
    - `kgstv_decode.py`, `tone_demod.py` — düşük seviye demod/framing yardımcıları
    - `decode_session.py` — **ana araç**: bir WAV kaydını baştan sona decode edip PNG üretir
    - `jpeg_block.py` — JPEG 16×16 blok decode referans implementasyonu (Python)
    - `kgstv_filters.py` — `kgstv.exe`'den disassembly ile çıkarılan gerçek FIR filtreleri
- `samples/` — gerçek off-air kayıtlar (TA4SO, TA2VB, DL4CQ) ve kontrollü test kayıtları
  (OH2UDS), referans görüntüleriyle birlikte
  - `samples/decoded/` — decode edilen çıktı PNG'leri
- `src/kgstv-core/` — Rust çekirdek kütüphane (cargo workspace üyesi), platform-bağımsız
  protokol implementasyonu: framing, konvolüsyonel kod, whitening, CRC, header, **JPEG codec**

## Geliştirme

```sh
# Rust çekirdeğini test et
cargo test --workspace

# Python prototip decoder: bir WAV kaydını PNG'ye decode et
cd research && source venv/bin/activate
python3 decoder/decode_session.py ../samples/<kayit>.wav ../samples/decoded/<cikti>.png
```

## Sonraki Adımlar

Bkz. proje görev listesi ve `docs/protocol_spec.md`'nin ilgili bölümleri:
1. 4L-FSK için MLSD/Viterbi-tabanlı CPFSK demodülatörü (basit sembol-bazlı algılama
   yapısal olarak yetersiz kaldı — ton aralığı ortogonal değil)
2. Rust'ta ses I/O + modülatör/demodülatör (MSK) katmanını yaz — `jpeg` codec'i artık hazır,
   sıradaki eksik parça bu
3. Platform UI'ları (Rust çekirdek + platform-native arayüz, kullanıcı tercihi)
