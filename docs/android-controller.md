# Android’den bilgisayar kontrolü (0.7.0)

Bu, Mac’ten Android’i açan ADB/scrcpy adaptöründen ayrı, telefona/tablete kurulan native
bir Android uygulamasıdır. ADB veya scrcpy gerektirmez. Aynı LAN üzerinde mevcut Noise
protokolünü ve eşleştirme v2’yi kullanır; bulut hesabı gerekmez.

| Yön | Destek |
| --- | --- |
| Android → Windows | H.264 ekran, mutlak dokunmatik fare, sürükleme, sağ tık, iki parmak kaydırma, fiziksel klavye ve Unicode metin |
| Android → Mac | Göreli dokunmatik yüzey, tıklama/kaydırma, fiziksel klavye ve Unicode metin; **Mac ekranı/panosu aktarılmaz** |
| Mac → Android | Önceki ADB/scrcpy akışı korunur; [ayrı kurulum](android.md) |

## Kurulum

- [Android APK](https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-android.apk):
  Android 8.0+ (API 26), **ARM64 veya x86_64**. 32 bit ARM cihazlar bu pakete dahil değildir.
- Mac ve Windows uygulamalarını 0.7.0’a güncelleyin. 0.6.2 Windows alıcısıyla ekran/HID giriş
  çalışabilir, fakat yeni Unicode metin özelliği 0.7.0 yeteneğinin doğrulanmasını gerektirir.
- Android’de indirilen APK için tarayıcı/dosya yöneticisinin normal yükleme onayını verin.
  Mikrofon, ekran kaydı veya Android Erişilebilirlik servisi gerekmez.
- Aynı bilgisayarı kullanan diğer denetleyicileri **önce duraklatın**. Sunucu tek etkin giriş
  sahibi kullanır; başka bir Mac’in otomatik yeniden bağlanması Android oturumunu kapatabilir.

APK sürüm güncellemeleri aynı yerel imza anahtarıyla imzalanır. Genel sertifika SHA-256:

```
5722354f306ad9f68d96c92421d1d9f446fa443de157cf123a25f3ef831c3185
```

Bu, Android paket imzasıdır; Play Store dağıtımı veya Apple/Microsoft yayıncı imzası değildir.
APK dosyasının SHA-256 özeti ayrıca sürümün `SHA256SUMS` dosyasındadır.

## Windows’a bağlanma

1. Windows’ta 0.7.0 alıcısını açın. Oturum açık, ana monitör etkin olmalı.
2. Android’de **Cihaz ara → bilgisayar → Eşleştir** seçin.
3. Android ve bilgisayarda görünen parmak izinin **sekiz grubunun tamamını** karşılaştırın.
   Yalnız hepsi aynıysa bilgisayarda **İzin ver** seçin. Android eşleştirme penceresinde
   **İptal** isteği kapatır; uygulama arka plana giderse istek iptal edilir.
4. Android’in **Cihazlarım** listesinden **Bağlan** seçin.
5. Görüntü geldikten sonra tek dokunuş sol tık, basılı tutma sağ tık, tek parmak sürükleme
   fare sürükleme, iki parmak hareketi kaydırmadır. Fiziksel klavye de kullanılabilir.
6. **Klavye → Yaz** Unicode metni doğrudan bilgisayara yazar; pano üzerinden geçirmez.
   Esc, Tab, Enter ve Ctrl+C/V düğmeleri standart HID kısayollarıdır.

Ekran en fazla 1920×1080, 30 FPS ve 8 Mbps olarak istenir; donanım/ağ gerçek sonucu sınırlar.
Monitör en-boy oranı korunur; dokunmatik koordinatlar yalnız görüntünün içinden hesaplanır.
İlk çözülmüş kare gelmeden ekrana giriş gönderilmez. UAC/giriş güvenli masaüstü, ses,
dosya aktarımı, çoklu monitör seçimi ve otomatik pano paylaşımı bu sürüme dahil değildir.

## Mac’e bağlanma

1. Mac’te 0.7.0 menüsünden **Android’den bu Mac’i kontrol et… → Alıcıyı başlat** seçin.
2. Mac uygulaması için Erişilebilirlik ve Yerel Ağ izinleri gerekli. Alıcı varsayılan olarak
   kapalıdır ve uygulama yeniden açıldığında kendiliğinden başlamaz.
3. Mac’ten başka bilgisayara gönderim duraklatılır; aynı uygulamanın alıcı ve gönderici
   modlarını aynı anda kullanması engellenir. Enjekte edilen olaylar yeniden iletilmez.
4. Android’de **Cihaz ara → Mac → Eşleştir** seçin. İki cihazdaki sekiz grubu karşılaştırıp
   **Mac’te** izin verin; ardından Android’de **Bağlan** seçin.
5. Android’de görüntü yerine açıkça **yalnız dokunmatik yüzey/klavye** modu gösterilir.
   Tek parmak göreli fare hareketi, dokunuş sol tık, basılı tutma sağ tık ve iki parmak
   kaydırma sağlar. Mac ekranına doğrudan bakarak kullanılır.
6. Mac menüsündeki aynı alıcı öğesi sunucuyu kapatır. **Mac alıcı erişimleri…** bir cihazı
   veya tüm cihazları iptal eder; bu yönetim alıcı kapalıyken de kullanılabilir.

Mac alıcısı ayrı `mobile-host.toml` ve `mobile-host.access.toml` dosyaları kullanır;
Mac’in mevcut gönderici eşleştirmeleri değiştirilmez. Alıcıyı kapatmak açık oturumları
keser; kayıtlı yetkileri silmez. Mac’in yeniden başlatılması/göndericiye dönülmesi sonrası
alıcıyı bilerek yeniden başlatmanız gerekir.

## Keşif ve yaşam döngüsü

- Keşif Android NSD ile `_keyboard-it._tcp.` hizmetini bulur. İsim/IP/ilan edilen kimlik
  yalnız adres ipucudur; kayıtlı kriptografik kimlik Noise Hello sonrasında doğrulanır.
- Ağ multicast’i engelliyorsa **IP ile ekle** kullanın. Eşleştirme ve masaüstü portları
  varsayılan 5600/5601’dir; çakışmada değişebilir, bilgisayarın gerçek portlarını kullanın.
- DHCP adresi değişirse **Cihaz ara** ile tekrar keşfedin. Yeni adres yalnız mevcut
  anahtar ve sabit kimlik doğrulandıktan sonra kaydedilir.
- Uygulama arka plana gidince, ekran yüzeyi kapanınca, cihaz değişince veya bağlantı
  kopunca oturum kapatılır. Yeniden bağlanma kullanıcı tarafından başlatılır; eski giriş
  kuyruğu başka bir cihaza taşınmaz. Host da soket kapanınca tuttuğu tuş/fareyi bırakır.
- Ekran oturumu Android’in `FLAG_SECURE` korumasını kullanır; ekran görüntüleri/uygulama
  önizlemesi siyah görünebilir. Bu, uzaktaki bilgisayarın görüntüsünün gelmediği anlamına gelmez.

## Güvenlik

Native Rust/JNI kitaplığı mevcut `protocol` crate’ini kullanır; Android için yeni bir Noise
uygulaması yazılmamıştır. Eşleştirme v2 tam 256 bit parmak izi, sonraki oturumlar eşleştirme
anahtarı ve sabit cihaz kimliği ile doğrulanır. Pencereyi kapatmak/arka plana geçmek bekleyen
isteği iptal eder; eski sonuçlar yeni cihazın kaydını değiştiremez.

Android kayıtları Android Keystore’daki dışarı çıkarılamayan AES-256 anahtarıyla AES-GCM
kullanılarak şifrelenir. Taze IV ve bağlı veri kullanılır; bozulmuş kayıt sessizce boş kayıtla
değiştirilmez. Android yedekleme/cihaz aktarımı kapalıdır. Anahtarlar Intent, günlük veya
panoya yazılmaz. Root/ele geçirilmiş uygulama süreci ve işletim sistemi/codec açıkları için
mutlak güvenlik iddiası yoktur. Native PC/Mac anahtar dosyaları önceki güvenlik modelini korur.

Unicode metin isteği yeni, doğrulanmış `text_input` yeteneğini gerektirir. 4096 bayt üstü,
NUL ve geçersiz UTF-8 reddedilir. Eski alıcıya bilinmeyen kayıt gönderilmez. Mobil görüntü
boyutu ve H.264 erişim birimi 4 MiB ile, komut/video kuyrukları sabit kapasitelerle sınırlıdır.
Kodlanmış referans kareleri atılmaz: taşmada oturum kapanır ve yeni bağlantı/anahtar kare gerekir.

**Cihazı Unut** yalnız Android kaydını siler; bilgisayardaki yetkiyi iptal etmek için alıcı
ayarlarını kullanın. İptal, mevcut soketi kapatır ve yeni girişin dispatch edilmesini engeller.
Ağdaki kötü niyetli bir tarafın paketleri engelleyerek hizmeti kesmesi önlenmiş sayılmaz.

## Derleme ve doğrulama

```sh
# JDK 21, Android SDK 35/build-tools 35.0.0, NDK 27.2.12479018
rustup target add aarch64-linux-android x86_64-linux-android
ANDROID_HOME=/path/to/sdk packaging/android/build.sh debug
cd android && ./gradlew testDebugUnitTest assembleDebugAndroidTest
```

Dağıtım için `packaging/android/sign-release.py` yerel `.android-signing/` dizinindeki
kalıcı anahtarı kullanır; dizin Git’ten dışlanır. Anahtarı ve parola dosyasını özel biçimde
yedekleyin: kaybında mevcut kurulum üzerine aynı imzayla güncelleme yapılamaz. CI yalnız
**test imzalı** APK üretir; bu dosya kamu sürümüne otomatik eklenmez. Native Mac/Windows ve
Android CI başarılı olduktan sonra yerel release APK’sı `publish-apk.py v0.7.0` ile eklenir.

Rust testleri gerçek şifreli loopback oturumlarında eşleştirme, kimlik uyuşmazlığı, iptal,
video birleştirme/boyut sınırları, Mac giriş modu, Unicode ve erişim iptalini sınar.
Android birim testleri Annex-B SPS/PPS ayrıştırmasını ve HID kayıtlarını sınar.
`NativeSessionTest`, izole Android emülatöründen JNI → Rust Noise → fixture sunucu →
Android MediaCodec yolunu, Keystore kayıt bütünlüğünü ve kapanmış JNI handle reddini sınar:

```sh
cargo run -p controller-client --example mobile_fixture --release
# Emulator connects to host loopback through 10.0.2.2; no physical OS input is used.
cd android && ./gradlew connectedDebugAndroidTest
```

Fixture yalnız `127.0.0.1:57811/57812/57821/57822` üzerinde çalışır ve gerçek giriş adaptörü
kullanmaz. Fixture’ın otomatik onayı üretim alıcılarında yoktur.

4 Kasım 2026’da yerel doğrulama: **120 Rust testi**, **2 Android birim testi**, **5 Android
15 ARM64 emülatör testi**, Windows/Linux çapraz derleme ve Mac gizli arayüz kontrolleri geçti.
Beş emülatör testi; gerçek H.264 çözme, Android NSD keşfi, JNI hata/handle sınırları,
Mac giriş/Unicode/Keystore bütünlüğü ve Activity’den dokunma/arka plana geçiş yolunu kapsadı.
İlk UI testindeki süresiz `waitForIdleSync` beklemesi, gerçek hazır/kapanmış durumunu
süre sınırıyla bekleyen doğrulamayla değiştirildi; başarı koşulları kaldırılmadı.

**Fiziksel Android telefon/tablet → gerçek Windows/Mac denemesi henüz yapılmadı.** Android
Wi-Fi keşfi, farklı donanım codec’leri, dokunmatik sürükleme, klavye yerleşimleri, uyku/uyanma
ve uzun oturum performansı ayrıca fiziksel kabul testine tabidir. Android’den Mac ekranını
görmek için ayrıca bir macOS ekran yakalama/kodlama altyapısı geliştirilmelidir.
