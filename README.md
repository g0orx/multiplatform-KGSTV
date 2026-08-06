# KGSTV — Cross-Platform Yeniden Yazım

**Amaç:** JJ0OBZ (K.G., Niigata-Myoko, Japonya) tarafından yazılan, sadece Windows'ta çalışan
**KG-STV** (dijital SSTV) yazılımının; Windows, Linux, macOS ve (mümkünse) iOS/Android'de
çalışan, havadaki orijinal KG-STV istasyonlarıyla uyumlu bir yeniden yazımını yapmak.

## Durum

Protokol, orijinal `readme.txt` çevirisi + gerçek off-air ses kayıtları üzerinde bit-seviyesinde
ters mühendislikle **büyük ölçüde doğrulandı**. Ayrıntılar: [`docs/protocol_spec.md`](docs/protocol_spec.md).

| Katman | Durum |
|---|---|
| Framing (preamble/sync/frame-walk) | ✅ Gerçek kayıtla doğrulandı |
| Konvolüsyonel kod (K=7, 171/133 oktal) | ✅ Doğrulandı, Rust'ta implement edildi |
| Whitening (127-bit M-sequence) | ✅ Doğrulandı, Rust'ta implement edildi |
| CRC (CRC-16/AUG-CCITT) | ✅ Doğrulandı, Rust'ta implement edildi |
| MSK demodülasyon | ✅ Üretim kalitesinde (Python prototip) |
| JPEG Huffman/entropy tabloları | ✅ Standart tablolar bit-uyumlu doğrulandı |
| JPEG kuantizasyon ölçeği (`sc`→scale) | ⚠️ Açık |
| 4L-FSK demodülasyon kalibrasyonu | ⚠️ Açık |
| Rust çekirdeği (`src/kgstv-core`) | 🚧 Framing+conv+whiten+CRC+header hazır, testli |
| Platform UI'ları | ⏳ Henüz başlanmadı |

## Dizin Yapısı

- `docs/` — araştırma ve protokol dokümantasyonu
  - `research.md` — web araştırması bulguları
  - `protocol_spec.md` — **otoriter kaynak**: tam protokol spesifikasyonu + doğrulama kayıtları
- `research/` — ters mühendislik çalışma alanı
  - `binaries/` — orijinal `kgstv.zip` (KGSTV.exe, readme'ler, kgstv.ini)
  - `sampleaudio/` — gerçek off-air ses kayıtları (MSK/4LFSK, Norm/Convolution, farklı JPEG scale)
  - `decoder/` — Python prototip decoder (`kgstv_decode.py`, `tone_demod.py`, `frame_walker.py`,
    `jpeg_block.py`) — protokolü doğrulamak için kullanılan çalışan kod
- `src/kgstv-core/` — Rust çekirdek kütüphane (cargo workspace üyesi), platform-bağımsız
  protokol implementasyonu

## Geliştirme

```sh
# Rust çekirdeğini test et
cargo test --workspace

# Python prototip decoder (ses analizi için)
cd research && source venv/bin/activate
python3 decoder/frame_walker.py sampleaudio/wav/KG-STV_MSK_Norm.wav
```

## Sonraki Adımlar

Bkz. proje görev listesi ve `docs/protocol_spec.md`'nin "Hâlâ Belirsiz" bölümleri:
1. JPEG `sc`→kuantizasyon-ölçeği eşlemesini kesinleştir
2. 4L-FSK demodülasyon kalibrasyonu (gerçek düz-metin veya `kgstv.exe` disassembly ile)
3. Rust'ta ses I/O + modülatör/demodülatör (MSK, ileride 4L-FSK) katmanını yaz
4. Rust'ta JPEG 16×16 blok codec'ini yaz (encode+decode)
5. Platform UI'ları (Rust çekirdek + platform-native arayüz, kullanıcı tercihi)
