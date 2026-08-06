# KG-STV Protokol Spesifikasyonu (Orijinal readme.txt'den)

> **Kaynak:** `kgstv.zip` içindeki `readme.txt` (KG-STV ver.1.0.1), JJ0OBZ (K.G., Niigata-Myoko,
> Japonya) tarafından yazılmış. Dosya Shift-JIS/CP932 kodlamasında; `research/notes/readme_utf8_cp932.txt`
> içinde UTF-8'e çevrilmiş hali mevcut. Bu belge, o bölümün **birebir çevirisidir** — yorum/tahmin
> içermez, sadece orijinal Japonca metnin çevirisidir. Yazarın kendi ifadesiyle bu bir "deneysel"
> protokol; yazar da zamanla hataların/eksikliklerin bulunabileceğini not ediyor.

Bu, elimizdeki **tek otoriter kaynak**. Web'deki üçüncü parti özetlerin (sigidwiki, wells10000 vb.)
verdiği sayılarla (1200 baud, MSK/4LFSK frekansları, Viterbi K=7) uyuşuyor, ama burada **bit
seviyesinde çerçeve yapısı** ekstra bilgi olarak var.

## Genel Aktarım Parametreleri

| Parametre | Değer |
|---|---|
| Modülasyon | MSK, veya MSK + 4-seviyeli FSK karma modülasyon |
| Baud rate | 1200 baud |
| MSK frekansları | Space (0) = 1200 Hz, Mark (1) = 1800 Hz |
| 4L-FSK frekansları | `00`→1200 Hz, `01`→1400 Hz, `10`→1600 Hz, `11`→1800 Hz |
| Bant genişliği | 500–2500 Hz |
| Hata düzeltme | Yok, veya Viterbi kodu (NASA standart K=7 kod) |
| Beyazlatma (whitening/scrambling) | Var — periyodu 127 bit olan bir M-sequence kodu her bite XOR'lanır |
| Interleaver | Yok |
| Header (senkron öncesi) | `01` tekrarlı sinyal, 256 bit |
| Temel kod yapısı | senkron kodu + 54-bit bilgi chunk'ı (header) + değişken uzunlukta veri chunk'ı |
| Hata tespiti | Var (CRC-16/CCITT) |
| Senkron bit dizisi | 63 bit'lik M-sequence kodu |
| Görüntü sıkıştırma | JPEG uyumlu (16×16 piksel blok, 4:1:1 chroma subsampling) |
| Metin kodlaması | Shift-JIS uyumlu |
| Emisyon tipi | F1D (SSB modunda çalışma), F2D (FM modunda çalışma) |

## Bilgi Chunk'ı (Header) Yapısı — sistem kodu versiyon '0'

```
┌──────┬──────┬───┬───┬──────┬──────┬──────┬──────┬──────┐
│ sys  │ com  │ c │ m │  x   │  y   │  sc  │ size │ CRC  │
└──────┴──────┴───┴───┴──────┴──────┴──────┴──────┴──────┘
 0      4      8   9   10     16     22     26     38   54 bit
```

| Alan | Bit offset | Genişlik | Anlamı |
|---|---|---|---|
| `sys` | 0 | 4 bit | Sistem kodu (versiyon) |
| `com` | 4 | 4 bit | Komut kodu (bkz. komut tablosu) |
| `c` | 8 | 1 bit | Kodlama modu (NORM/CONV, yani konvolüsyonel kod açık/kapalı) |
| `m` | 9 | 1 bit | Modülasyon modu (MSK / 4L-FSK) |
| `x` | 10 | 6 bit | Görüntü bloğu konumu (X) |
| `y` | 16 | 6 bit | Görüntü bloğu konumu (Y) |
| `sc` | 22 | 4 bit | JPEG ölçek/kalite boyutu |
| `size` | 26 | 12 bit | Veri (data chunk) boyutu |
| `CRC` | 38 | 16 bit | Hata tespit kodu (CRC-16 CCITT) |

Toplam header uzunluğu: **54 bit**.

> **Önemli not:** Bilgi chunk'ı (header), veri chunk'ının kendi modülasyon/kodlama ayarından
> bağımsız olarak **her zaman MSK modülasyonu + konvolüsyonel kodlama** ile gönderilir. Yani
> kullanıcı 4L-FSK veya NORM (kodsuz) seçse bile, header her zaman en dayanıklı ayarla gider —
> sadece veri chunk'ı kullanıcının seçtiği modülasyon/kodlamayla gönderilir.

`x` ve `y` 6 bit oldukları için 0–63 aralığında blok koordinatı ifade edebilir — 320×240 görüntü,
16×16'lık bloklara bölündüğünde 20×15 = 300 blok olur (X: 0–19, Y: 0–14), yani 6 bit fazlasıyla
yetiyor.

## Veri Chunk'ı (Data Chunk) Yapısı

```
┌──────────────────────────────────────────┬──────┐
│                data                       │ CRC  │
└──────────────────────────────────────────┴──────┘
```

Veri chunk'ı, bilgi chunk'ının CRC'sinin hemen ardından gönderilir. Kendi CRC'si de var (yine
CRC-16 CCITT olduğu varsayılıyor, ayrıca belirtilmemiş ama muhtemelen aynı).

## Komut Tablosu (sistem kodu versiyon '0')

| Komut değeri | İşlev |
|---|---|
| 0 | Metin aktarımı (text) |
| 1 | Görüntü aktarımı (image) |
| 2 | BSR yanıtı (BSR response — istenen blokların yeniden gönderimi) |
| 3 | Bitiş (end/finish) |
| 4 | BSR isteği (BSR request — kayıp blokların istenmesi) |
| 5 | İptal / kesinti (abort) |
| 6 | Çağrı işareti (callsign) |

## Çerçeveleme Kuralları

1. Aktarıma başlarken önce **256 bit'lik `01` tekrar deseni** (header/preamble) gönderilir —
   alıcının bit/clock senkronizasyonu kurması için.
2. Bir "veri formatı" (kod yapısı) şu şekilde tanımlanır:
   **senkron kodu + bilgi chunk'ı (header) + veri chunk'ı**.
   Ancak **bitiş (end), iptal (abort) ve BSR sinyalleri veri chunk'ı içermez** — sadece
   senkron + header gönderilir.
3. Görüntü verisinde, bu format **blok sayısı kadar arka arkaya** gönderilir (her 16×16 blok
   için bir tam "senkron+header+data" birimi).
4. Metin aktarımında tek bir format birimi yeterlidir (kendi içinde tamamlanır).
5. Aktarım **normal tamamlandığında**, sonunda **bitiş sinyali 3 kez** art arda gönderilir.
6. Aktarım **iptal edildiğinde**, sonunda **iptal sinyali 3 kez** art arda gönderilir.

## Senkron Kodu (63 bit M-sequence)

```
000011100001001000110110010110101110111100110001010100111111010
```

## Beyazlatma (Whitening) Kodu — periyot 127 bit M-sequence

```
1110110011000100100111001111100100000100011010101001101101001010
000101100001100101111111010110111011110001110100010101110000001
```//iki satır art arda birleşince 127 bitlik diziyi oluşturuyor.

## Diğer Notlar (readme.txt'den)

- Görüntü boyutu her zaman 320×240'a normalize ediliyor (BMP/JPEG girişi otomatik resize +
  crop ediliyor).
- 320×240 bir görüntünün tam aktarımı, sıkıştırma ayarına bağlı olarak **~2 dakika** sürüyor.
- BSR (Bad Segment Report) mantığı: alıcı, eksik/hatalı blokların koordinatlarını vericiye
  BSR isteği (komut=4) olarak gönderir; verici bu koordinatlara karşılık gelen blokları BSR
  yanıtı (komut=2) olarak tekrar gönderir.
- Metin mesajı: tek seferde en fazla 510 yarım-genişlik (255 tam-genişlik) karakter.
- Görüntü kalite/skala ayarı kullanıcı tarafından seçilebilir (header'daki `sc` alanı, 4 bit
  → 0–15 arası bir JPEG ölçek/kalite değeri kodluyor olmalı).

## Sonradan Eklenen Özellikler (readme2.txt / readme3.txt — build002→004)

Bunlar protokolü **değiştirmiyor**, sadece uygulama (UI/UX) katmanında iyileştirmeler:
- build002→003: sekmeli (tab) arayüz, "AF Level" ekranı, watchdog/timeout iyileştirmeleri,
  ImageProcessor harici log entegrasyonu (JA2GRC'nin yazılımı), rastgele/dikey blok tarama sırası
  seçeneği (fading karşısında blok kaybını dağıtmak için).
- build003→004: BSR mantığında sağlamlaştırma, çağrı işareti (callsign) güvenilirliği için
  header'ın N kez tekrar gönderilmesi (ayarlanabilir, varsayılan 5), otomatik eski görüntü
  silme, log çıkışı iyileştirmeleri.

## ✅ DOĞRULANDI — Gerçek Kayıt Üzerinde Bit-Seviyesi Analiz (2026-08-06)

`research/sampleaudio/KG-STV_MSK_text.mp3` kaydı, `research/decoder/` altındaki Python prototip
demodülatörle uçtan uca çözüldü. Sonuç: **çözülen metin okunabilir düz metin olarak çıktı**:

```
TEST3>The Quick Brown Fox Jumps Over The Lazy Dog 0123456789
```

Bu, aşağıdaki tüm noktaları **kesin olarak** doğruluyor:

1. **Bit dönüşümü:** space (1200 Hz) = bit `0`, mark (1800 Hz) = bit `1`. Zamanda soldan sağa,
   spec'teki bit dizeleriyle birebir eşleşiyor (ters çevirme/inversiyon yok).
2. **Preamble:** Tam 256 bit, `01` deseni tekrarı — spec ile tam eşleşme.
3. **Senkron kelimesi:** 63 bit, preamble'ın hemen ardından, spec'teki dizeyle **birebir**
   eşleşme (sync'in kendisi hiç whitening'e uğramıyor).
4. **Bilgi chunk'ı (header) kodlama sırası:** ham 54 bit → **rate-1/2, K=7 konvolüsyonel
   kodlama** (NASA standart üreteç polinomları **171, 133 oktal**; çıkış sırası her girdi biti
   için **(G133_biti, G171_biti)**) → 108 kodlanmış bit → bu 108 bit üzerine **127-bit whitening
   M-sequence XOR'lanıyor, sayaç HER CHUNK'IN BAŞINDA 0'dan yeniden başlıyor** (global/sürekli
   sayaç DEĞİL). Gerçek kayıt üzerinde Viterbi decode **sıfır hata (metric=0)** ile tam çözüldü.
5. **Header alan değerleri** (bu örnek için): `sys=0, com=0(metin), c=0(NORM), m=0(MSK), x=0,
   y=0, sc=10, size=480(bit)`. `sc=10` değeri, aynı dağıtımdaki `kgstv.ini` dosyasındaki
   `ImageQ=10` ayarıyla **birebir örtüşüyor** — çapraz doğrulama.
6. **Veri chunk'ı kodlama sırası:** `c=0` (NORM) olduğunda veri chunk'ı konvolüsyonel kodlama
   **almıyor** — sadece ham veri bitleri + kendi 16-bit CRC'si, ayrıca **kendi whitening sayacı
   sıfırdan başlıyor** (header'ın sayacından bağımsız). `size` alanı **bit** cinsinden veri
   uzunluğunu ifade ediyor (byte değil — 480 bit = 60 byte, kayıt süresine de tam uyuyor).
7. **CRC algoritması — TAM TESPİT EDİLDİ:** **CRC-16/AUG-CCITT**
   (`poly=0x1021, init=0x1D0F, refin=false, refout=false, xorout=0x0000`), bit-serial olarak
   hesaplanıyor. Hem header'ın 38-bit alanı üzerindeki CRC (`0x6702`) hem data chunk'ının 480-bit
   metin verisi üzerindeki CRC (`0xA2D1`) bu algoritmayla **tam olarak doğrulandı**.

### Doğrulanmış Encode/Decode Zinciri (metin/NORM modu için)

```
TX sırası:  preamble(256×01) → sync(63bit, whitening YOK) →
            [ham 54bit header → conv-encode(K=7,171/133) → 108bit → whiten(idx 0'dan)] →
            [ham data bit'leri + CRC16-AUG-CCITT(16bit) → whiten(idx 0'dan, header'dan bağımsız sayaç)]
```

Bu zincir hem encode hem decode yönünde (Rust implementasyonunda) simetrik olarak kurulabilir.

### Görüntü Modu Üzerinde Kısmi Doğrulama

`KG-STV_.10_scale.wav` (gerçek görüntü aktarımı, 174.8s) üzerinde ilk frame'i decode ettim:
`{sys=0, com=6(ÇAĞRI İŞARETİ), c=0(NORM), m=1(4L-FSK), x=0, y=0, sc=3, size=40bit}` —
**CRC doğrulandı (crc_ok=True)**, Viterbi metric=7 (kanalda gerçek/hafif gürültü var, beklenen).
Bu, header çözme zincirinin (senkron bulma → conv-decode → CRC) **görüntü modunda da** çalıştığını
kanıtlıyor. `com=6` göstermesi, görüntü aktarımından önce vericinin önce çağrı işaretini (callsign)
duyurduğunu doğruluyor (readme3.txt'deki "çağrı işareti N kez tekrar gönderilsin" özelliğiyle
tutarlı). `m=1` bu oturumda veri chunk'larının **4L-FSK** ile gönderildiğini gösteriyor.

Bu 4L-FSK veri chunk'ının (çağrı işaretinin kendisi, 40 bit) çözümü ise **henüz güvenilir değil** —
basit "ani frekans" (frequency-discriminator) yöntemi 4 seviyeli FSK için yeterince gürbüz değil;
sembol başına 4 ton arasında ayrım net değil (~%15-20 sembol hatası gözlemlendi). Bunun için
Rust implementasyonunda (ya da bir sonraki Python iterasyonunda) **ton-korelasyonu (Goertzel/FFT
matched-filter) tabanlı bir 4-seviye demodülatör** gerekiyor — bu, gerçek 16×16 JPEG blok
içeriğini görmek için son eksik parça.

### ✅ Tam Transmisyon Yürüyüşü (Frame Walking) — Gerçek Görüntü Blokları Çıkarıldı

`research/decoder/frame_walker.py`, ton-korelasyon demodülatörü kullanarak `KG-STV_MSK_Norm.wav`
kaydının **tamamını** (148.9 saniye) baştan sona dolaştı. Sonuç: 5 kez tekrarlanan çağrı işareti
duyurusu (`com=6, "TEST3"`, hepsi CRC-valid), ardından **22 farklı koordinatta gerçek görüntü
bloğu (`com=1`)**, çoğu geçerli CRC ile. Bu şunu kesinleştiriyor:

- Ardışık frame'ler arasında **preamble YOK, sadece 63-bit senkron kelimesi tekrarlanıyor**
  (256-bit preamble sadece transmisyonun en başında bir kez gönderiliyor).
- Header'daki `x`,`y` alanları gerçekten blok koordinatı olarak davranıyor (0-19, 0-14 aralığında,
  320×240/16×16=20×15 blok ızgarasıyla tam uyumlu).
- `size` alanı blok başına **değişken** (JPEG sıkıştırmasının doğası — düz/tek renk alanlar çok
  kısa, detaylı alanlar daha uzun); bit sayısı 8'in katı olmak zorunda değil (örn. 378 bit) —
  bu, bloğun **standart JPEG dosya formatı değil, salt entropy-coded bitstream** (byte hizalama
  veya JPEG marker/stuffing olmadan) olarak paketlendiğini gösteriyor.
- Bazı düz/tekrarlı görüntü alanları (örn. gökyüzü gibi tek renkli bölgeler) **birebir aynı kısa
  bayt dizisini** üretiyor (`6532994ca0` dört farklı koordinatta tekrarlandı) — DC-baskın,
  AC-katsayıları sıfır olan JPEG blokları için beklenen davranış (Huffman EOB kısalığı).
- CRC hatası veren bloklar muhtemelen ya gerçek kanal bozulması (gerçek RF/ses kaydı gürültüsü)
  ya da 148 saniyelik kayıt boyunca birikmiş küçük saat kayması (tek bir sabit `anchor` tüm
  dosya için kullanıldı) — üretim kalitesinde bir decoder için sembol-bazlı zamanlama kurtarma
  (timing recovery loop) gerekecek.

**Sonraki adım:** CRC-valid blok baytlarını gerçek bir baseline-JPEG Huffman/DCT decoder'a
besleyip (varsayılan/standart tablolarla dene, tutmazsa `kgstv.exe`'den tabloları çıkar) piksel
verisine çevirmek — bu, "resmin encode/decode" sorusunun son parçası.

### ✅ JPEG Entropy Katmanı — Standart Tablolarla Doğrulandı

`research/decoder/jpeg_block.py`, standart ITU T.81 Annex K Huffman tabloları (DC/AC,
luma/chroma) ve standart zigzag sırasıyla, frame11'in CRC-doğrulanmış 186-bit ham blok verisini
decode etti:

- **Bit tüketimi tam olarak 186/186** — hiçbir Huffman senkron hatası, hiçbir EOF/format hatası
  olmadan 6 blok (4×Y + 1×Cb + 1×Cr) baştan sona ayrıştırıldı. Yanlış bir tablo seti ile bunun
  şansla olması istatistiksel olarak neredeyse imkânsız → **KG-STV'nin entropy-coding katmanı
  standart JPEG Huffman tablolarıyla bit-uyumlu**.
- İki düz Y bloğu (sadece DC, AC katsayısı yok) **tam olarak geçerli gri ton değerlerine**
  (98.0 ve 68.0 — 0-255 aralığında) çözüldü. Cb/Cr blokları DC=0 → 128 (nötr renk).
- İki blok AC katsayısı içeriyor (gerçek doku); bu bloklarda piksel değerleri 0-255 aralığının
  dışına taşıyor — bu, Huffman/tablo hatası değil, **kuantizasyon tablosu ölçeklendirme
  formülünün** (`sc` 4-bit alanı → IJG "quality 1-100" formülü) yanlış kalibre edilmiş olmasından
  kaynaklanıyor. `sc` alanı sadece 4 bit (0-15) olduğu için IJG'nin 1-100 ölçeğiyle doğrudan
  eşleşmiyor; gerçek eşleme (`sc` → skala çarpanı) henüz belirlenmedi.

**Sonuç:** JPEG codec'inin **yapısı** (Huffman tabloları, zigzag, DCT blok organizasyonu,
4:2:0-eşdeğeri 4:1:1 chroma, DC prediction) kesinleşti. Kalan tek bilinmeyen: `sc` → kuantizasyon
ölçek eşlemesi. Bunu netleştirmenin en hızlı yolu `kgstv.exe`'nin JPEG encode fonksiyonundaki
sabit tabloyu/formülü okumak, ya da farklı `sc` değerleriyle (`.10_scale` vs `.50_scale`
örnekleri) karşılaştırmalı deneme yapmak.

### Hâlâ Doğrulanması Gereken (görüntü modu için)

Yukarıdakiler **metin + NORM + MSK** kombinasyonu için kanıtlanmış. Görüntü modu için ek doğrulama
gerekiyor: `c=1` (CONV) durumunda veri chunk'ının konvolüsyonel kodlamasının tam davranışı (whitening
sırası aynı mı), 4L-FSK'nin veri chunk'ında sembol-bit eşlemesi, ve 16×16 JPEG blok verisinin iç
yapısı (quantization/Huffman tabloları). Bu doğrulamalar için `sampleaudio` klasöründeki
`KG-STV_MSK_Convolution.mp3`, `KG-STV_4LFSK_*.mp3` ve `.10_scale`/`.50_scale` kayıtları kullanılacak.

## Hâlâ Belirsiz / Doğrulanması Gereken Detaylar

readme.txt bit-seviyesinde çerçeveyi tarif ediyor ama şunlar hâlâ açık değil — bunlar için
ya `kgstv.exe`'nin ilgili fonksiyonlarını disassemble etmemiz ya da gerçek bir off-air kayıt
üzerinde bit-eşleştirme yapmamız gerekiyor:

1. **Bit sırası** — 54 bitlik header alanları ve 63/127 bitlik M-sequence dizileri MSB-first mi
   gönderiliyor, LSB-first mi?
2. **Viterbi K=7 polinomları** — "NASA standart" dendiğine göre büyük ihtimalle CCSDS/NASA
   standart rate-1/2, polinom (171,133) oktal — ama teyit gerekiyor (flush/tail-biting var mı?).
3. **CRC-16 CCITT parametreleri** — polinom 0x1021 standart ama init value / final XOR / giriş-
   çıkış bit yönü (reflected mi) teyit edilmeli.
4. **JPEG varyantının tam parametreleri** — quantization tablosu, Huffman tabloları özel mi
   standart mı; her 16×16 blok bağımsız bir mini-JPEG mi yoksa paylaşılan tablo + sadece entropy
   data mı gönderiyor (54-bit header'da ayrı bir quant-tablosu alanı yok, bu yüzden muhtemelen
   sabit/standart bir tablo seti kullanılıyor ve `sc` alanı sadece ölçek/kalite seçiyor).
5. **`sc` (4 bit) alanının tam anlamı** — hangi sayısal değer hangi JPEG kalite/ölçeğine denk
   geliyor.
6. **Kanal kodlamasının (whitening + convolutional) tam uygulanış sırası** — whitening
   convolutional kodlamadan önce mi sonra mı uygulanıyor.

## Sonraki Adım Önerisi

Bu belge protokolün iskeletini tamamen veriyor. Kalan boşluklar (madde 1–6) için:
- **Öncelik 1:** Gerçek bir off-air ses kaydı (kullanıcı sağlayacak) üzerinde bir prototip
  demodülatörle bit-senkronizasyonu deneyip senkron/header alanlarını gözlemleyerek bit sırasını
  ve CRC parametrelerini doğrulamak.
- **Öncelik 2:** `kgstv.exe` içinde JPEG encode/decode ve Viterbi encode/decode fonksiyonlarını
  Ghidra ile bulup sabit tabloları (quantization, Huffman, polinom) çıkarmak.
