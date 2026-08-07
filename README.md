# Atölye m-KGSTV — Cross-Platform Yeniden Yazım

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
| **Rust ses I/O + MSK modülatör/demodülatör** (`kgstv-modem`) | ✅ Uçtan uca doğrulandı: gerçek OH2UDS MSK kaydında 300/300 tile, sıfır bit hatasıyla decode edildi |
| **4L-FSK MLSD/Viterbi demodülatörü** (`kgstv-modem::fsk4`) | ✅ Çözüldü — 6 durumlu CPM faz-trellis Viterbi + ince zamanlama kayması taraması; gerçek kayıtta %96-99.7 tile başarı oranı (önceki ~%80 sembol-doğruluğu tıkanmasından büyük sıçrama) |
| **Masaüstü GUI** (`kgstv-gui`, egui/eframe) | ✅ Win/Linux/macOS tek kod tabanı — **RX**: WAV aç, ilerlemeli tile çizimi, frame günlüğü, canlı (mikrofon) decode, göz diyagramı, MSK+4L-FSK otomatik; **TX**: resim/metin gönder (MSK ya da 4L-FSK), WAV kaydet veya hoparlörden çal |
| iOS/Android UI | ⏳ Henüz başlanmadı |

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
- `src/kgstv-modem/` — Rust ses katmanı (cargo workspace üyesi, `kgstv-core` üzerine inşa
  edilir): WAV I/O (`wav`), bant-geçiren ön-filtre (`filter`), gerçek sürekli-faz MSK
  modülasyon/demodülasyon + ton-korelasyonu bit algılama (`msk`), **4L-FSK MLSD/Viterbi
  modülasyon/demodülasyon** (`fsk4` — 6 durumlu CPM faz-trellis), senkron kelimesi arama
  (`sync`), üst seviye TX/RX oturum API'si (`session`, header/senkron her zaman MSK + veri
  chunk'ı `hdr.m`'ye göre MSK/4L-FSK otomatik, ilerleme callback'li encode dahil), varsayılan
  mikrofon/soundcard'dan sürekli örnek yakalama + hoparlörden çalma (`live`, `cpal` üzerinden).
  `examples/decode_wav.rs`: bir WAV dosyasını uçtan uca decode edip PPM görüntü üreten CLI aracı.
- `src/kgstv-gui/` — masaüstü GUI (cargo workspace üyesi, egui/eframe), RX ve TX modları:
  **RX** — WAV dosyası aç (native dosya seçici, `rfd`) ya da canlı (mikrofon) decode başlat,
  arka planda `kgstv-modem` ile decode et (MSK/4L-FSK otomatik ayırt edilir), tile'ları geldikçe
  canlı çiz, frame günlüğünü göster; **TX** — bir resim dosyası (`image` crate ile
  PNG/JPEG/GIF/BMP) 320×240'a ölçekleyip 300 tile JPEG'e + isteğe bağlı çağrı işareti/metine
  encode eder (MSK ya da 4L-FSK modülasyonu seçilebilir), WAV olarak kaydeder ya da doğrudan
  hoparlörden çalar. Her iki modda sinyal kalitesi/zamanlamasını değerlendirmek için göz
  diyagramı (eye diagram) gösterilir. Windows/Linux/macOS için tek kod tabanı.

## Geliştirme

```sh
# Rust çekirdeğini (+ modem katmanını) test et
cargo test --workspace

# Rust CLI: gerçek/sentetik bir MSK WAV kaydını uçtan uca decode et (PPM görüntü üretir)
cargo run -p kgstv-modem --release --example decode_wav -- samples/<kayit>.wav çıktı.ppm

# Masaüstü GUI'yi çalıştır
cargo run -p kgstv-gui --release

# Python prototip decoder: bir WAV kaydını PNG'ye decode et
cd research && source venv/bin/activate
python3 decoder/decode_session.py ../samples/<kayit>.wav ../samples/decoded/<cikti>.png
```

## Sonraki Adımlar

Bkz. proje görev listesi ve `docs/protocol_spec.md`'nin ilgili bölümleri:
1. `kgstv-modem::session`'daki ardışık (sıralı) frame yürüyüşünü, gürültülü gerçek off-air
   kayıtlar için `decode_session.py`'deki bağımsız-aday-tarama yaklaşımına taşı (uzun/gürültülü
   kayıtlarda senkron kaybında sıralı yürüyüş durur)
2. 4L-FSK'nin kalan birkaç tile hatası (287-299/300, muhtemelen gerçek kanal gürültüsü) için
   daha geniş/ince faz+zamanlama taraması ya da yumuşak-karar (soft-decision) CRC/CONV
   birleşimi denenebilir — kısmi iyileştirme, blokaj değil
3. iOS/Android UI (egui zaten Android/iOS derlemesini destekliyor — masaüstü kod tabanı taşınabilir
   olmalı; platform-özel ses I/O entegrasyonu ayrıca gerekir)
