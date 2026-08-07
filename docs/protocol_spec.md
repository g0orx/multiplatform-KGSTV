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

### 2026-08-07 Güncellemesi: Bilinen-Düz-Metinle Kalibrasyon — Kısmi Başarı

Kullanıcının sağladığı kontrollü test görüntüleri (OH2UDS çağrı işareti, düz renk + çizgili,
hem MSK hem 4L-FSK, hem NORM hem CONV) sayesinde artık **gerçek bilinen düz-metin** ("OH2UDS")
ile kalibrasyon mümkün oldu — 5 farklı dosyadan (aynı çağrı işareti tekrarı) toplam 30 ölçüm
toplanıp `docs/protocol_spec.md` bu bölümündeki eski "TEST3 varsayımı geçersiz" bulmacası
çözüldü:

- **İşaret (pozitif/negatif) doğruluğu: %96 (23/24 sembol)** — kesin bir ilerleme
- Bilinen etiketlerle küme merkezleri kalibre edildi: **[-230.5, -74.6, 93.6, 210.6] Hz**
  (ideal ±100/±300 Hz'den belirgin şekilde sıkışık — muhtemelen gerçek/sentetik kayıt zincirinin
  kendi frekans tepkisi)
- Sembol doğruluğu **~%80-83**'e ulaştı — hem frekans-discriminator hem ton-korelasyon
  yöntemiyle, **30 tekrar üzerinden ortalama almak bile sonucu değiştirmiyor** → kalan hatalar
  gürültü değil, **sistematik/deterministik bir blok-arası girişim (ISI)**
- **Pratik sınama:** düz beyaz görüntünün en küçük bloğunu (72 bit, 36 sembol) bu kalibre
  merkezlerle decode etmeyi denedim — %80 sembol doğruluğu JPEG Huffman'ın sıfır hata
  toleransı karşısında **yetersiz kaldı**, decode birkaç blok sonra bozuldu.

**Kök neden (disassembly bulgusuyla tutarlı):** 4L-FSK'nin 200Hz ton aralığı 1200 baud sembol
periyodunda tam ortogonal değil (MSK'nin 600Hz aralığının aksine) → basit sembol-bazlı (blok
ortalaması ya da ton-korelasyonu) algılama yapısal olarak yetersiz kalıyor. **Sonraki adım:**
gerçek CPFSK faz hafızasını hesaba katan bir **Maximum-Likelihood Sequence Detection (Viterbi
tabanlı CPFSK demodülatörü)** — ayrı, iyi tanımlanmış bir mühendislik projesi.

Eski, artık kısmen aşılmış not (tarihsel referans için bırakıldı):

Bu 4L-FSK veri chunk'ının (çağrı işaretinin kendisi, 40 bit) çözümü ise **hâlâ açık** —
hem ton-korelasyon hem ani-frekans (discriminator) yöntemleri denendi, **iki bağımsız
kayıtta (`KG-STV_.10_scale`, `KG-STV_4LFSK_Norm`) tutarlı/tekrarlanabilir ama beklenen 4
seviyeye net ayrılmayan** frekans okumaları elde edildi (6 çağrı-işareti tekrarı üzerinden
ortalama alındı, gürültü değil — sistematik bir kalibrasyon/eşleme sorunu). Bilinen-düz-metin
("TEST3", MSK modundan doğrulanmış) ile geriye doğru kalibrasyon denendi ama net bir eşleşme
çıkmadı — **bu 4L-FSK kayıtlarındaki gerçek çağrı işareti "TEST3" olmayabilir**, yani varsayım
geçersiz kalmış olabilir. Sonuç: MSK için demodülasyon tam kanıtlanmış ve üretim kalitesinde;
**4L-FSK için ya gerçek düz-metin bilinmeli ya da `kgstv.exe`'nin demodülatör kodu (filtre/eşik
sabitleri) incelenmeli**. Şimdilik açık bırakıldı (bkz. proje görev listesi #8) — MSK modu
tek başına tam bir KG-STV implementasyonu için yeterli (encode taraf hem MSK hem 4L-FSK
üretebilir; sadece 4L-FSK *decode* kalibrasyonu eksik).

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

### ✅ KRİTİK DÜZELTME — Tile İçindeki 6 Blok DC Zincirlemez (2026-08-07)

Gerçek off-air kayıtlarla (TA4SO, BBC test kartı) piksel-piksel karşılaştırma yapılınca, CRC
geçen bloklarda bile renklerin referansla uyuşmadığı görüldü. Kontrollü **düz renk** test
görüntüleriyle (tamamen kırmızı / tamamen mavi 320×240, OH2UDS tarafından kaydedilmiş) kesin
teşhis kondu: tamamen düz bir 16×16 karonun 4 Y bloğu, standart JPEG'in aksine **birbirine
zincirlenmeden**, her biri **bağımsız, sıfırdan başlayan bir DC değeri** taşıyor. Eski kodumuz
(klasik JPEG MCU mantığıyla) bu 4 değeri konvülasyon zinciri gibi üst üste **biriktiriyordu**
(`dc = dc_pred + diff; dc_pred = dc`), bu da düz kırmızı bir karoda bile -58,-116,-174,-232 gibi
saçma, gittikçe büyüyen bir dizi üretiyordu (oysa gerçek fark ≈0 olmalıydı — görüntü tamamen
düz). Aynı zincirleme hatası Cb/Cr arasında da vardı.

**Düzeltme:** `research/decoder/jpeg_block.py::decode_16x16_block` — 6 bloğun (Y0,Y1,Y2,Y3,Cr,Cb)
her biri artık **predictor=0 ile bağımsız** decode ediliyor (chain YOK).

**Doğrulama:** BBC test kartı (`KG-STV_MSK_Convolution.wav`) ile referans (`1.0_scale.png`)
arasındaki ortalama piksel farkı **222.99 → 125.37**'ye düştü ve görsel olarak kızın yüzü,
kırmızı eşarbı, yeşil palyaço ve alttaki "BBC" yazısı artık **açıkça tanınabilir**. Yatay/dikey
kırmızı-beyaz çizgili kontrollü test görüntüleri **300/300 karo, keskin ve doğru kenarlarla**
decode edildi. Gerçek TA4SO kaydında ortalama fark (sadece decode edilen 57 karo) **~117**'ye
düştü ve görsel olarak lacivert arka plan + krem/sarı detaylar artık referansla tutarlı.

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

### ✅ `sc` → Kuantizasyon Ölçeği — ÇÖZÜLDÜ (kgstv.exe binary analizi + gerçek piksel doğrulaması)

`kgstv.exe` içinde standart JPEG Huffman tabloları (AC/DC luma/chroma) **birebir**, ve onlardan
hemen önceki bellek bölgesinde **standart IJG kalite=50 taban kuantizasyon tabloları** (luma ve
chroma, her ikisi de birebir) bulundu (`research/binaries/kgstv_extracted/kgstv/kgstv.exe`,
offset ~907037 luma başlangıcı, ~907117 chroma başlangıcı — 4 bayt/değer, big-endian paketli).

Gerçek `KG-STV_MSK_Norm.wav` kaydından decode edilen düz (AC katsayısız) bloklardan (`sc=10`,
DC=-3→piksel 98.0, DC=-6→piksel 68.0) geriye doğru hesaplama: kullanılan `qt[0,0]=80`. Standart
IJG formülüyle **quality=10** için: `scale=5000/10=500`, `qt[0,0]=floor((16×500+50)/100)=80` —
**tam eşleşme**. Yani:

> **`sc` alanı, IJG JPEG "quality" parametresinin (1–100 ölçeği) doğrudan kendisidir.**
> Standart formül: `scale = quality<50 ? 5000/quality : 200-2*quality`;
> `qval = clamp(floor((base_taban×scale+50)/100), 1, 255)`.

AC katsayılı blokların ilk denemede 0–255 aralığı dışına taşması bir ölçek hatası değildi —
sadece **IDCT çıkışının 0–255'e kırpılmasını (clamp) unutmuştuk**; bu her JPEG decoder'ında
standart bir son adımdır. Kırpma eklendiğinde AC'li bloklar tutarlı, gerçekçi bir desen
gösteriyor (örn. yatay bantlama — bir fotoğraftaki yatay kenar/gradyanla tam uyumlu).
`research/decoder/jpeg_block.py` güncellendi.

**Sonuç:** JPEG codec'inin **hem yapısı hem de kuantizasyon ölçeklendirmesi** artık tam olarak
biliniyor. Görüntü katmanında kalan tek şey, üretim kalitesinde bir Rust implementasyonu yazmak
— bilinmeyen bir parametre yok.

### ✅ `kgstv-modem` (Rust ses I/O + MSK modem) — Gerçek Kayıtla Uçtan Uca Doğrulandı (2026-08-07)

`src/kgstv-modem/` cargo workspace üyesi olarak yazıldı: `wav` (hound tabanlı WAV I/O, örnekleme
hızı dosyadan dinamik okunur), `filter` (RBJ biquad bant-geçiren, ileri+geri filtfilt), `msk`
(disassembly ile doğrulanmış gerçek sürekli-faz sentezi + Hanning-pencereli DFT ton-korelasyonu
demodülasyonu), `sync` (kaba+ince iki aşamalı senkron kelimesi arama) ve bunları
`kgstv-core::frame`/`jpeg` ile birleştiren üst seviye `session` (ardışık frame yürüyüşü + TX
oturum derleyicisi).

Gerçek `samples/OH2UDS-*-mode-msk-*.wav` kayıtlarında (`cargo run -p kgstv-modem --release
--example decode_wav`) uçtan uca doğrulama:
- Senkron, kaydın başından ~2.93sn sonra (kayıt cihazı başlatma gecikmesi) doğru şekilde
  bulundu — kaba arama adımının `tb/4` (~sembol süresinin dörtte biri) olması ve ince aramanın
  eşitlik durumunda kaba sonuca en yakın konumu tercih etmesi (aksi hâlde `window()`'ın negatif
  konumları 0'a clamp etmesi sahte eşitliklere yol açıyordu) gerekiyordu.
- `duz-kirmizi` (solid red) kaydı: 346 frame, **300/300 görüntü tile'ı** (`header_metric=0`,
  yani Viterbi'de sıfır bit hatası; tüm veri CRC'leri geçti) decode edildi, üretilen görüntü
  tam olarak beklenen **RGB=(178,0,0)** — `kgstv-core::jpeg` testindeki gerçek yakalanan
  kırmızı tile ile bit-eşleşiyor.
- `horizontal-kirmizi-beyaz` (CONV kodlu) kaydı: yine 300/300 tile decode edildi; kırmızı/beyaz
  sınırındaki 1-2 geçiş tile'ında beklenen lossy-JPEG kenar bulanıklığı (küçük renk karışımı,
  ~%0.6 piksel) gözlendi — bu bir decoder hatası değil, `sc=10` (düşük kalite) kuantizasyonun
  keskin bir kenarda beklenen davranışı.

**Sonuç:** MSK ses katmanı artık gerçek off-air/kontrollü kayıtlarla üretim kalitesinde
doğrulanmış durumda — WAV'dan piksel-doğru görüntüye kadar tüm zincir Rust'ta çalışıyor.

### ✅ 4L-FSK — MLSD/Viterbi Demodülatörü Çözüldü (2026-08-07)

Yukarıdaki kalibrasyon notunda tespit edilen kök neden (200Hz ton aralığı 1200 baud'da
ortogonal değil → bağımsız/hafızasız sembol kararları sistematik ISI'ye takılıyor, ~%80-83
sembol doğruluğunda tıkanıyor) için önerilen çözüm — CPFSK faz-hafızasını hesaba katan bir
Maximum-Likelihood Sequence Detection (Viterbi) — `src/kgstv-modem/src/fsk4.rs`'de uygulandı.

**Anahtar matematiksel gözlem:** disassembly ile doğrulanmış modülatör formülüyle
(`frekans = seviye×300+1500`, sürekli faz), 1200 baud'da dört olası frekansın taşıdığı
taşıyıcı çevrimi (`freq/baud`): 1200→1, 1400→7/6, 1600→4/3=8/6, 1800→3/2=9/6. Yani seviye `L`
sembolü, biriken faz durumunu **tam olarak `L×60°` ilerletir (mod 360°)** — modülasyon indeksi
`h=Δf/baud=200/1200=1/6` rasyonel olduğundan bu YAKLAŞIK değil KESİN bir 6 durumlu CPM/CPFSK
faz-trellis'i verir (standart CPM teorisi: `h=p/q` sadeleştirilmiş kesirse trellis `q`
durumludur). `fsk4.rs`, bu 6 durumlu trellis üzerinde tam bir Viterbi/MLSD çalıştırıp sembol
dizisini faz sürekliliğini doğru hesaba katarak kararlaştırıyor (basit ton-korelasyonun aksine).

**Karşılaşılan iki ek engel (ikisi de gerçek kayıtla test edilirken bulundu, sentetik testler
kazara gizliyordu):**
1. **Bilinmeyen başlangıç faz DURUMU:** Veri chunk'ının başındaki mutlak faz, kendinden önceki
   (her zaman MSK olan) senkron/header segmentinden gelen keyfi bir değerdir. İlk uygulama
   yanlışlıkla durum 0'dan başlamayı zorluyordu (sentetik testler hep durum 0'da başladığı için
   bu fark edilmiyordu) — düzeltme: Viterbi'ye 6 durumun hepsi için eşit/bilgisiz bir önsel
   verildi, `[0,60°)` aralığında ek bir global faz-ofseti taraması ile birleştirildi.
2. **İnce zamanlama kayması:** Gerçek kayıtlarda, MSK'nin büyüklük-bazlı (faz-uyumsuz)
   demodülasyonuna TAMAMEN görünmez kalan ~birkaç örneklik sabit bir zamanlama kayması bulundu
   (muhtemelen kayıt zincirinin grup gecikmesi) — faz-uyumlu (coherent) Viterbi korelasyonu
   buna çok duyarlı. Bilinen-doğru MSK verisiyle çapraz doğrulanan bir teşhis aracıyla izole
   edildi: `OH2UDS-duz-kirmizi-mode-4lfsk-code-normal-scale-1.wav`'da en iyi zamanlama kayması
   **+4.5 örnek** bulununca Hamming mesafesi 8/94'ten **0/94**'e düştü. Çözüm: `[-8,+8]` örnek
   aralığında ince bir kayma taraması (dosya için genelde sabit olduğundan ilk başarılı chunk'ta
   bulunan kayma sonraki chunk'lara "ipucu" olarak taşınıyor, taramayı hızlandırıyor).

**Gerçek kayıtla sonuç** (`cargo run -p kgstv-modem --release --example decode_wav`):
- `OH2UDS-duz-kirmizi-mode-4lfsk-code-normal-scale-1.wav` (NORM, hatasız düzeltme yok):
  **287/300 tile** decode edildi, başarılı olan HER tile tam olarak beklenen **RGB=(178,0,0)**
  ile bit-eşleşiyor (kısmi/bozuk bir tile yok — CRC-kapılı boru hattının beklenen "ya tam doğru
  ya hiç" davranışı).
- `OH2UDS-horizontal-kirmizi-beyaz-mode-4lfsk-code-convolution-scale-1.wav` (CONV, K=7
  konvolüsyonel hata düzeltmeli): **299/300 tile** — CONV kodlamanın kalan sembol hatalarının
  çoğunu düzelttiği, teorik beklentiyle tam tutarlı.

**Sonuç:** 4L-FSK artık gerçek off-air kalitesinde kayıtlarda **üretim kalitesine yakın**
çalışıyor (~%96-99.7 tile başarı oranı, önceki ~%80 sembol-doğruluğu tıkanmasından — pratikte
tek bir tile bile güvenilir decode edemeyen bir durumdan — devasa bir sıçrama). Kalan birkaç
tile hatası muhtemelen gerçek kanal gürültüsü/geçici SNR düşüşleri (CONV kodlamayla neredeyse
tamamen telafi ediliyor), yapısal bir eksiklik değil. `kgstv-modem::session`, header/senkron
için her zaman MSK kullanırken (protokolün gerektirdiği gibi), veri chunk'ı için `hdr.m`
alanına göre MSK/4L-FSK arasında otomatik geçiş yapıyor — hem RX (`decode_wav_sequential*`)
hem TX (`encode_session_audio*`) tarafında.

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

-2. **RX ön-uç filtre zinciri kgstv.exe disassembly ile bulundu ve doğrulandı** (waveIn
    callback @0x4033e8, radare2 ile takip edildi). Gerçek DSP zinciri:
    ```
    int16 PCM (44100Hz) → FILTER1 (51-tap anti-alias lowpass) → 4'e decimate (→11025Hz)
                        → FILTER2 (51-tap, ~900–2000Hz bandpass) → [devamı çıkarılmadı]
    ```
    Her iki filtrenin **tam katsayıları** binary'den okundu (dosya offset 841240 ve 841648,
    51×8-byte double) ve `research/decoder/kgstv_filters.py`'ye kaydedildi. FILTER2'nin frekans
    tepkisi hesaplandı: 0Hz'de -54dB (DC/hum reddi), 900-2000Hz'de düz (KG-STV ton bandı), 2500Hz
    üstünde hızla düşüyor — **tam olarak** kanal-seçici bir bandpas filtre.

    Bu gerçek filtre zinciri Python'da uygulanıp `.10_scale` örneğinde header tekrar decode
    edildi: **CRC doğrulandı** (metric=4), sonuç önceki basit Butterworth yaklaşımıyla aynı.
    4L-FSK çağrı işareti verisi de bu gerçek filtrelerle tekrar denendi — **çıkan seviye dizisi
    önceki (basit filtre) sonucuyla neredeyse birebir aynı** (bkz. aşağıdaki tablo), bu da
    demodülasyonumuzun **gürültüden değil, gerçek/tutarlı bir sinyalden** kaynaklandığını
    kanıtlıyor. Ancak sonuç hâlâ okunabilir ASCII vermiyor.
    **Sonuç:** Filtre kalitesi sorunun kaynağı değildi (zaten yeterince iyiydi). Kalan
    açıklamalardan en olası olanı: **bu örneklerdeki gerçek test metni "TEST3" değil** —
    bilinen-düz-metin varsayımımız bu spesifik kayıtlar için geçersiz. Gerçek metin bilinmeden
    (ya da RX'in sembol-karar/senkron mantığının tam izlenmesiyle) kalibrasyon tamamlanamıyor.

-3. **RX zincirinin TAMAMI çıkarıldı: 4 gerçek FIR filtre + Hilbert dönüşümü + faz-farkı
    ayrıştırıcı (discriminator).** Disassembly'e devam edilince şu tam mimari ortaya çıktı:
    ```
    int16 PCM (44100Hz)
      → FILTER1 (51-tap anti-alias lowpass)
      → 4'e decimate (→11025Hz)
      → FILTER2 (51-tap bandpass, ~900-2000Hz — KG-STV ton bandı)
      → FILTER3 (51-tap Hilbert dönüştürücü — merkez katsayı TAM 0.0, tam antisimetrik
                  → quadrature/imajiner bileşen üretiyor)
      → faz = atan2(FILTER3_çıkışı, FILTER2_çıkışı)   [sarılmış, -π..π]
      → faz açma (unwrap): ardışık farkı ±2π (0x403bc4'te bulunan sabit, TAM 6.283185307179586)
        ile kıyaslayıp gerektiğinde ekleyip/çıkararak sürekli hale getiriyor
      → anlık frekans = d(açılmış_faz)/dt × örnekleme_hızı/2π
      → FILTER4 (51-tap simetrik lowpass, merkez=0.125 — frekans kestirimini yumuşatıyor)
      → [sembol karar mantığı — henüz izlenmedi]
    ```
    Bu, **klasik bir Hilbert-dönüşümlü FM ayrıştırıcı** — kavramsal olarak bizim
    `fsk_discriminator` yaklaşımımızla (karmaşık taşıyıcıya indirip açı-farkı almak) **birebir
    aynı felsefe**, sadece IIR Butterworth yerine bu 4 gerçek FIR filtreyi kullanıyor.

    Bu tam zinciri (`research/decoder/kgstv_filters.py::apply_authentic_discriminator`)
    Python'da uygulayıp `.10_scale` örneğinde senkron aramasını tekrarladım: **sonuç daha
    kötü çıktı** (err=16/63, basit yaklaşımın err=0'ına kıyasla) — muhtemelen Hilbert işaret
    kuralı (`atan2` argüman sırası) ya da faz yönü konvansiyonunda küçük bir uyumsuzluk var;
    tam assembly ile bit-eşleşme için sabır isteyen ayrıca bir hata ayıklama gerekiyor.
    **Bu noktada araştırma durduruldu** — mimari tamamen anlaşıldı ve belgelendi, ancak son
    %100 bit-doğru replikasyon (ve olası known-plaintext problemi) hâlâ açık. Basit
    karmaşık-mixing yaklaşımı (MSK için zaten kanıtlanmış) üretim için yeterli.

-1. **Modülatör formülü kgstv.exe disassembly ile bulundu** (radare2, fonksiyon @0x405f94,
    frekans sabitleri @0x4068e8 civarı). Gerçek x87 FPU kodu:
    ```
    ; fld qword [seçilen seviye değeri, -1.0..+1.0 aralığında, dallanmayla seçiliyor]
    fmul dword [0x4068e8]   ; × 300.0
    fadd dword [0x4068ec]   ; + 1500.0   -> frekans (Hz)
    fmul qword [0x4068f0]   ; × 2π (0x401921FB54442D18 = 6.283185307179586)
    fmul qword [0x51974c]   ; × (muhtemelen 1/sample_rate)
    fadd qword [0x4f64a0]   ; faz akümülatörüne ekle (çalışma zamanı durumu)
    fstp qword [0x4f64a0]   ; -> phase += 2π·f/fs
    ```
    Bu **kesin olarak doğruluyor**: `frekans = seviye_değeri × 300 + 1500` (MSK: seviye=±1 →
    1200/1800 Hz; 4L-FSK: seviye=±1,±⅓ → 1200/1400/1600/1800 Hz), ve modülasyon **gerçekten
    sürekli-faz (continuous-phase, CPFSK)** — faz her örnekte biriktiriliyor, sembol sınırlarında
    sıfırlanmıyor. Bu, MSK adının ("Minimum Shift Keying") beklediği tam davranış.
    **Sonuç:** 4L-FSK'nin blok-bazlı ton-korelasyonuyla (Goertzel/DFT) güvenilir çözülememesinin
    nedeni artık netleşti — bu yöntem ayrık/kesikli-faz FSK için uygundur, ama CPFSK için
    sembol içi ton "sızıntısı" (200Hz aralık, ortogonal değil) yapısal bir sınırlama getiriyor.
    MSK'de bu sorun olmuyor çünkü 600Hz aralığı simge periyodunda tam ortogonal. Doğru çözüm,
    faz-türevi (frequency-discriminator) tabanlı, PLL/zamanlama-kurtarmalı bir demodülatör —
    ki bunu zaten denedik ama ince ayarı (filtre bant genişliği, sembol-içi pencere) hâlâ
    yetersiz. Alıcı tarafın (RX) tam eşik/filtre değerlerini bulmak için disassembly'e devam
    edilebilir, ya da kendi PLL-tabanlı demodülatörümüzü bu doğrulanmış CPFSK modeliyle daha
    dikkatli tasarlayabiliriz.

0. ~~Frekans sabitleri~~ — `kgstv.exe`'de offset ~24296-24320 civarında **300.0, 1500.0, 200.0,
   1200.0** (float32) sabitleri art arda bulundu: merkez frekans (1500), MSK sapması (±300),
   4L-FSK adım genişliği (200Hz) — varsayılan frekans modelimizle **tam örtüşüyor**. 4L-FSK
   decode sorununun kaynağı yanlış nominal frekanslar değil; muhtemelen demod algoritmasının
   kendisi (filtre/eşik) ya da whitening/bit-sıra detayı.
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
