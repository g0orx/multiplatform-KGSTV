# KG-STV — Araştırma Notları

## Proje Hedefi
JJ0OBZ tarafından yazılan, sadece Windows'ta çalışan **KG-STV** programının, orijinal protokolle
**havada uyumlu** çalışan; Windows, Linux, macOS ve (mümkünse) iOS/Android üzerinde çalışan bir
yeniden yazımını (rewrite) yapmak. Yani hedef sadece "benzer bir program" değil, gerçek KG-STV
istasyonlarıyla haberleşebilen bit-uyumlu bir implementasyon.

## KG-STV Nedir
KG-STV, dijital bir SSTV (Slow-Scan Television) modudur. Ses kartı üzerinden (soundcard-based)
telsiz ile görüntü ve kısa metin mesajları göndermeyi/almayı sağlar. Analog SSTV'nin aksine
görüntüyü JPEG olarak sıkıştırıp parçalar halinde dijital modülasyonla gönderir.

- **Geliştirici:** JJ0OBZ (Japonya, amatör radyo operatörü, "K.G.")
- **Görüntü boyutu:** 320×240 (klasik analog SSTV modlarıyla aynı)
- **Blok yapısı:** Görüntü 16×16 piksellik bloklara bölünüp her biri ayrı ayrı JPEG sıkıştırılıp
  gönderiliyor. Bu sayede iletim kesintiye uğrasa bile alınan bloklar görüntüde "delikli" ama
  görünür bir sonuç oluşturuyor.
- **Hata düzeltme / tekrar isteme:** Alıcı, hatalı/kayıp blokları **BSR (Bad Segment Report)**
  adlı bir geri bildirim mesajıyla vericiye bildiriyor; verici sadece o blokları yeniden gönderiyor.
- **Metin mesajlaşma:** Görüntülere ek olarak kısa metin mesajları da desteklemektedir.

## Modülasyon Detayları
İki modülasyon modu var:

| Mod | Açıklama | Hız | Not |
|---|---|---|---|
| **MSK** | Minimum-Shift Keying | 1200 Bd | Daha yavaş, daha stabil (fading'e dayanıklı) |
| **4LFSK** | 4-Level Frequency-Shift Keying | 1200 Bd (2 bit/sembol → efektif 2x veri) | Daha hızlı, propagasyon bozulmalarına daha hassas |

**MSK frekansları:** 0 → 1200 Hz, 1 → 1800 Hz
**4LFSK frekansları:** 00→1200 Hz, 01→1400 Hz, 10→1600 Hz, 11→1800 Hz

- **Bant genişliği:** ~500–2500 Hz (sinyal koşullarına göre değişken)
- **Hata düzeltme kodu:** Viterbi convolutional coding, NASA standart **K=7**, isteğe bağlı
  (kullanıcı encoding'siz de gönderebiliyor)
- **Operasyon:** USB (Upper Sideband), amatör SSTV bantlarında

## Bilinen Frekanslar / Bantlar
- 3733 kHz, 7173 kHz, 14233 kHz (HF SSTV bantları)
- 10.489625 GHz üzerinden QO-100 (geostationary amatör transponder)

## Yazılımın Durumu
- **Kapalı kaynak (closed-source)**, sadece Windows'ta çalışıyor.
- Bilinen tek alım/gönderim programı JJ0OBZ'nin kendi yazılımı; başka bir implementasyon
  (açık kaynak dahil) şu ana kadar tamamlanmamış görünüyor.
- `sstv@groups.io` üzerinde bir GNU Radio decoder block'u yazma fikri tartışılmış ama sonuç
  bulunamadı.

## Eksik / Bilinmeyen Teknik Detaylar (ters mühendislik gerektiriyor)
Web'de bulunan hiçbir kaynak şu an için **tam protokol dokümantasyonu** içermiyor. Bit-uyumlu
bir implementasyon için netleştirilmesi gereken noktalar:

1. **Senkronizasyon / preamble dizisi** — alıcının bit senkronizasyonu ve blok sınırlarını nasıl
   bulduğu.
2. **Paket / header formatı** — her 16×16 blok gönderilirken hangi başlık bilgisi (blok
   koordinatı, checksum/CRC, mod bilgisi vb.) ekleniyor.
3. **16×16 blok içindeki JPEG varyantının tam parametreleri** — standart baseline JPEG mi,
   yoksa özelleştirilmiş bir DCT/quantization tablosu mu kullanılıyor.
4. **BSR (Bad Segment Report) paket formatı** — geri bildirim mesajının bit düzeyinde yapısı.
5. **Metin mesajlaşma çerçevesi (framing)** — metin paketlerinin resim paketlerinden nasıl
   ayırt edildiği.
6. **Viterbi K=7 tam parametreleri** — generator polinomları, puncturing (varsa).

Bu detaylar ancak şu yollarla çıkarılabilir:
- **Statik analiz:** KGSTV.exe binary'sinin disassemble edilmesi (Ghidra/IDA) — sabitler,
  tablo verileri, encode/decode fonksiyonları.
- **Dinamik analiz:** Gerçek KG-STV yayınlarının off-air ses kayıtları (WAV) üzerinde
  spektrogram/demodülasyon analizi yapılarak bit akışının doğrulanması.

## Kaynaklar
- [KG-STV - Signal Identification Wiki](https://www.sigidwiki.com/wiki/KG-STV)
- [M5AML - KG-STV, a digital SSTV system by JJ0OBZ](http://www.wells10000.plus.com/m5aml/kg-stv.htm)
- [What is KG-STV in Amateur Radio? - Johnson's Techworld](https://johnsonfrancis.org/techworld/what-is-kg-stv-in-amateur-radio/)
- [How to receive and decode KG-STV transmissions (sgcderek)](https://sgcderek.github.io/blog/kgstv-reception.html)
- [KG-STV MSK 1200Bd/600 via QO-100 (diario SWL I-56578)](http://i56578-swl.blogspot.com/2019/07/kg-stv-msk-1200bd600-via-qatar-oscar-100.html)
- [sstv@groups.io — open-source implementation of KG-STV?](https://groups.io/g/sstv/topic/open_source_implementation_og/80462530)

## Sonraki Adımlar
Bkz. proje görev listesi (task tracker) — sırada:
1. Ters mühendislik kaynaklarının toplanması (binary + kayıtlar)
2. Cross-platform teknoloji yığınına karar verme
3. Protokolün fiilen çıkarılması
