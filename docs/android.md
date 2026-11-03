# Android bağlantısı

Mac denetleyicisine **Cihazlarım… → Android Ekle…** akışı eklendi. Android cihazları
bilgisayarlarla aynı kayıtlı cihaz listesinde seçilebilir. Görüntü ve kontrol,
seçili cihaz için açılan ayrı bir **scrcpy penceresinde** çalışır. Uygulamanın Windows
masaüstü görüntüleyicisi Android video protokolünü doğrudan çözmez.

**26 Ekim 2026'da SM-G780G / Android 13 ile USB bağlantısı, uygulamadan cihaz ekleme
ve seçme, canlı görüntü ve fareyle kontrol doğrulandı.** Görüntü ve fare kontrolü
kullanıcı tarafından açık telefon penceresinde onaylandı. Wi-Fi eşleştirmesi, klavye
ve diğer cihaz modelleri henüz fiziksel olarak doğrulanmadı. Aşağıdaki akış bu
0.6.0 uygulamasına aittir; fiziksel testin tarihi bu sürümden öncedir.

## Araçlar

Bilgisayarda Android SDK Platform Tools içindeki `adb` ve güncel `scrcpy` bulunmalı.
Kurulum ve bağlantı gereksinimleri için [Android ADB belgesi](https://developer.android.com/tools/adb)
ve [scrcpy bağlantı belgesi](https://github.com/Genymobile/scrcpy/blob/master/doc/connection.md)
kullanılabilir. Araçlar otomatik kurulmaz veya indirilmez.

Uygulama PATH içindeki mutlak dizinleri, `/opt/homebrew/bin`, `/usr/local/bin`,
`ANDROID_HOME` / `ANDROID_SDK_ROOT` altındaki `platform-tools` dizinini ve macOS'ta
`~/Library/Android/sdk/platform-tools` konumunu kontrol eder. Finder'dan açılan uygulama
Homebrew araçlarını böyle bulabilir. Farklı kurulumlar için uygulamayı başlatırken
`KEYBOARD_IT_ADB` ve `KEYBOARD_IT_SCRCPY` değişkenlerine çalıştırılabilir dosyaların
**mutlak yollarını** verin. Geçersiz bir açık yol belirtilirse başka araca sessizce geçilmez.

## USB ile ekleme

1. Telefonda geliştirici seçeneklerini ve USB hata ayıklamayı açın.
2. Telefonu bilgisayara bağlayıp kilidini açın; telefondaki bu bilgisayara izin istemini onaylayın.
3. Mac'te **Cihazlarım… → Android Ekle… → Listeyi Yenile** seçin.
4. **Hazır** durumundaki telefonu seçip **Seçili Android'i Kaydet** düğmesine basın.
5. Cihazlarım listesinde telefonu seçip **Bu Cihaza Geç** deyin; aynı pencereden
   veya menüden **Ekranı Aç** seçin.

Geliştirme sırasında `cargo run -p mac-sender -- --devices` komutu Cihazlarım
penceresini doğrudan açar. Android seçiliyken macOS'un genel klavye yakalama izinleri
Android görüntüsünü ve scrcpy penceresindeki kontrolü engellemez.

`unauthorized` cihazlar telefondan izin bekliyor olarak, `offline` cihazlar çevrimdışı
olarak gösterilir. Bu durumlarda Kaydet kapalıdır. Kaydetmeden önce hazır olma durumu
tekrar kontrol edilir. İlk kayıt kendini seçer; başka bir cihaz aktifken Android eklemek
aktif bağlantıyı değiştirmez.

## Wi-Fi ile ekleme

Android 11 ve üzerindeki kablosuz hata ayıklama akışında telefon ile bilgisayarı
aynı Wi-Fi ağına bağlayın. Telefonda Kablosuz hata ayıklama bölümünü açın.

1. **Eşleştirme koduyla cihaz eşleştir** ekranındaki IP:port ve altı rakamlı kodu alın.
2. Android Ekle penceresinde **Eşleştirme IP:port** ve **Kod** alanlarını doldurun.
3. Kablosuz hata ayıklamanın ana ekranındaki **bağlantı IP:port** bilgisini ayrı alana yazın.
4. **Wi-Fi Eşleştir ve Bağlan** seçin; ardından listeden hazır telefonu seçip kaydedin.

İki port aynı olmak zorunda değildir. IP adresi ve açık port gerekir; adres alanı kabuk
komutlarını veya ADB seçeneklerini kabul etmez. IPv6 için `[IP]:port` biçimi kullanılır.
Eşleştirme kodu ADB'nin standart girdisine gönderilir; komut argümanına, kayıt dosyasına
veya hata mesajına yazılmaz. Eşleştirme ADB'de başarılı olup bağlantı portu hatalıysa,
telefonun güncel portunu kontrol edin; yeni kodla tekrar eşleştirmek de mümkündür.

**Wi-Fi güvenliği:** Android 11+ **Kablosuz hata ayıklama** eşleştirmesi TLS kullanır.
Eski `adb tcpip 5555` bağlantısı şifresizdir; güvenmediğiniz ağda kullanmayın. Uygulama
ADB'deki önceden açık bağlantıları da listelediğinden her Android Wi-Fi bağlantısının
TLS kullandığını garanti etmez. Mac–Windows için kullanılan Noise Android kanalını
sarmalamaz. Ayrıntı: [ADB Wi-Fi mimarisi](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md).

Daha önce ADB ile bağlanmış kablosuz cihazlar da Listeyi Yenile sonucunda görünür.
Telefonun bağlantı adresi değişirse güncel bağlantıyı listeden yeniden kaydedin.
Aynı Android kimliği için yeniden kaydetme mevcut yerel kaydı ve izinleri korur;
aktif oturum varsa kapanır ve yeniden **Ekranı Aç** gerekir.

## Kontrol ve izinler

- **Ekran** kapalıysa Android penceresi açılmaz.
- **Klavye / fare** kapalıysa scrcpy `--no-control` ile yalnız görüntüleme modunda açılır.
- Otomatik metin panosu eşitlemesi her Android oturumunda kapalıdır. Android için ilgili
  kutu devre dışıdır; bilgisayarlar için pano ayarı ayrı çalışmaya devam eder.
- scrcpy'nin kullanıcı tarafından başlatılan yapıştırma kısayolları korunur; otomatik pano
  kapatma, harici programın manuel yapıştırmasını engelleyen bir güvenlik sınırı değildir.
  Bu davranış [scrcpy kontrol belgesinde](https://github.com/Genymobile/scrcpy/blob/master/doc/control.md) açıklanır.
- Ses aktarımı kapalıdır. Android kontrolü kendi scrcpy penceresindedir; çift Fn ile
  genel klavye yakalama Android'e yönlendirilmez. Varsayılan scrcpy SDK klavye yolu
  kullanılır; tüm Unicode metinler için destek varsayılmamalıdır.
- Kalite profili FPS, uzun kenar sınırı ve belirtilmişse video bit hızına uygulanır;
  varsayılan uzun kenar tavanı 4096'dır. Profil değişiminden sonra **Ekranı Aç** gerekir.

Cihaz değiştirmek, aktif cihazı unutmak, izinlerini değiştirmek, bağlantıyı duraklatmak
ve uygulamadan çıkmak sahip olunan scrcpy sürecini kapatır. Yavaş biten eski bağlantı
hazırlıkları yeni seçimi açamaz; eski oturumun temizliği de yeni pencereyi kapatamaz.
Pencere kapandıktan veya bağlantı koptuktan sonra otomatik olarak başka cihaz açılmaz.
Eksik/eski araçlar, cihaz izni ve görüntüleme sürecinin hataları arayüzde gösterilir.

## Kimlik ve yetkilendirme

Android bağlantısı ADB'nin bu bilgisayar için verilmiş hata ayıklama yetkisini kullanır.
Native bilgisayar oturumlarının Noise anahtarları Android kayıtlarına taşınmaz.
Android kaydı açık seri/adres ve ADB üzerinden okunan Android kimliğini içerir. Her
başlatmada aynı seri seçilir; rastgele veya listedeki tek cihaza düşme yapılmaz. Kaydedilmiş
kimlik uyuşmazsa pencere açılmaz. Android kimliği ek bir adres yeniden kullanım kontrolüdür;
kriptografik doğrulamanın yerine geçmez ve kullanıcı/fabrika ayarları değişiminde yenilenebilir.

**Cihazı Unut** yalnız bu uygulamanın yerel kaydını siler. ADB'nin bilgisayar yetkisini,
anahtarlarını veya global sunucusunu kaldırmaz. Yetkiyi tamamen iptal etmek için telefonun
USB/kablosuz hata ayıklama ayarlarını kullanın. Oturum kapatılırken diğer ADB araçlarının
bağlantıları ve global sunucu sonlandırılmaz.

## Cihaz olmadan doğrulama

```sh
cargo test --workspace --locked
cargo run -p mac-sender --locked -- --check-devices
cargo run -p mac-sender --locked -- --check-viewer
cargo check -p android-adapter -p mac-sender -p win-receiver -p device-host --target x86_64-pc-windows-gnu --tests --locked
cargo check -p android-adapter -p protocol -p mac-sender -p device-host --target x86_64-unknown-linux-gnu --all-targets --locked
```

Testler sahte ADB çıktıları, kayıt geçişleri, kod gizliliği, kimlik uyuşmazlığı,
izinlerin komutlara uygulanması, eski oturumların reddi ve kontrollü yerel süreçlerin
zaman aşımı/kapatılması üzerinde çalışır. Native arayüz kontrolü Android kurulum
penceresini ve cihaz seçimini gizli kontrollerle sınar; ADB/scrcpy başlatmaz ve cihaz aramaz.

Wi-Fi eşleştirme, diğer OEM izinleri, ekran döndürme, Türkçe klavye, uyku/ağ kopması,
tutulmuş tuşların bırakılması ve gecikme/performans ayrıca doğrulanmalıdır.
Android'e özel uygulama, doğrudan uygulama içi Android video motoru, ses/dosya aktarımı,
Android'den başka cihazları kontrol etme ve Android TV'ye özel kumanda bu aşamaya dahil değildir.
