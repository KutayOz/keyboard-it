# keyboard-it — M0 Handoff (Windows Ajanı İçin)

> **Sana görev:** Bu belgedeki adımları izleyerek `keyboard-it` projesinin **M0** aşamasını Windows PC'de **derle**, **çalıştır** ve **çalıştığını doğrula**. Sonra "Bana ne rapor et" bölümündeki bilgileri geri gönder. Bu belge kendi kendine yeterlidir; önceki bir konuşmayı bilmene gerek yok.

---

## 1. Durum / bağlam

**Proje "keyboard-it":** Bir MacBook'un dahili klavyesiyle bu Windows PC'yi LAN üzerinden kontrol etmek. Donanım (dongle/kablo) yok, Bluetooth yok — iki bilgisayara da kendi yazılımı kurulur ve LAN üzerinden konuşurlar. Aslında bu, "yalnızca klavyeye indirgenmiş" özel bir Synergy/Deskflow uygulamasıdır ve tamamen **Rust** ile yazılmıştır. (Aktivasyon ileride "Fn'e çift basma" ile olacak — bu ileri bir kilometre taşı, seni şu an ilgilendirmiyor.)

Proje bir **Rust Cargo workspace**'idir ve 3 crate içerir:

| Crate | Ne işe yarar | Seni ilgilendiriyor mu? |
|---|---|---|
| `crates/protocol` | Paylaşılan tel formatı (iki tarafın ortak dili). OS'tan bağımsız. | Hayır (sadece derlenir) |
| `crates/mac-sender` | macOS tarafı (klavye yakalama). | **Hayır** — bu Mac tarafının işi |
| `crates/win-receiver` | Windows tarafı. **M0 binary'si budur.** | **EVET — çalıştıracağın bu** |

**M0'ın amacı:** Windows tarafının Win32 **`SendInput`** API'siyle (scancode kullanarak) klavye tuşu enjekte edebildiğini kanıtlamak. M0 binary'si, **2 saniyelik bir gecikmeden sonra** odaktaki pencereye harfi harfine `hello` yazar.

- **Ağ YOK.** TCP yok, Mac yok. Tamamen yerel, tek makinede çalışan bir test.
- Eğer ekranda `hello` belirirse, tüm sistemin **taklidi en zor yarısı** (sentetik tuş enjeksiyonu) kanıtlanmış olur.

**Önemli doğrulanmış gerçekler (Mac tarafında test edildi):**
- Workspace Mac'te sorunsuz derleniyor (`cargo build` + `cargo test -p protocol` geçiyor). Yani kaynak kod sağlam; senin işin Windows'a özgü derleme + çalıştırma.
- `windows` crate = **0.62.2**, feature'lar: `Win32_UI_Input_KeyboardAndMouse` + `Win32_Foundation`.
- `SendInput` bu sürümde **slice tabanlıdır** (`&[INPUT]` alır) — bu belgede önemli olacak (bkz. Sorun giderme).
- `hello` için PS/2 Set-1 scancode'ları kullanılıyor: `h=0x23, e=0x12, l=0x26, o=0x18`. Scancode'lar fiziksel tuş **konumunu** adresler, bu yüzden herhangi bir Latin/QWERTY düzeninde (US, TR-Q vb.) doğru çalışır.

**Ön koşul — proje klasörü:** Windows makinede `keyboard-it` klasörünün **mevcut olması gerekir** (kullanıcı Mac'ten kopyalar). Klasör yoksa, kod olmadan hiçbir şey yapamazsın — **kullanıcıdan `keyboard-it` klasörünü iste / temin et**, sonra devam et. Klasörün doğru olduğunu kökündeki `Cargo.toml` (içinde `[workspace]`) ve `crates/win-receiver/` alt klasörüyle teyit edebilirsin.

---

## 2. Ön koşullar (kurulum)

Hedef: Windows'ta çalışan bir **Rust + MSVC** araç zinciri. `windows-rs` crate'i için resmî olarak önerilen host üçlüsü `x86_64-pc-windows-msvc`'dir.

> **Not:** Derlemek ve çalıştırmak için **Yönetici (Administrator) gerekmez.** rustup kullanıcı profiline kurulur. (Yönetici yalnızca VS Build Tools yükleyicisi isterse veya bir kereye mahsus uzun yol ayarı için gerekebilir.)

### 2.1 Önce zaten kurulu mu diye bak

**Yeni bir PowerShell** açıp çalıştır:

```powershell
rustc --version
cargo --version
rustup show
```

- Üçü de sürüm bilgisi yazdırıyorsa Rust kuruludur. `rustup show` çıktısında **default host** olarak `x86_64-pc-windows-msvc` görüyorsan MSVC araç zincirindesin (istediğimiz bu). `...-gnu` görüyorsan GNU'dasın (o da çalışır, ama MSVC önerilir).
- `windows` 0.62.2 için **Rust 1.85+** gerekir. Emin olmak için:

```powershell
rustup update
```

Rust kuruluysa **2.3'e (MSVC linker kontrolü)** geç.

### 2.2 Rust yoksa: rustup kur

İki resmî yol var — biri yeterli:

```powershell
# Yol A (winget — Microsoft'un belgelediği komut):
winget install Rustlang.Rustup
```

veya

```
# Yol B: https://rustup.rs/ adresinden rustup-init.exe indir ve çalıştır,
# varsayılanları (Enter) kabul et. Varsayılan host üçlüsü otomatik olarak
# x86_64-pc-windows-msvc seçilir (istediğimiz budur).
```

rustup temiz bir makinede eksik MSVC ön koşullarını **algılar ve otomatik kurmayı teklif eder** (Visual Studio Community'yi çeker) — teklif ederse kabul edebilirsin.

> **KRİTİK:** Kurulumdan sonra **YENİ bir terminal aç.** Zaten açık olan terminaller/IDE'ler PATH değişikliğini görmez.

### 2.3 MSVC linker kontrolü — "link.exe not found" tuzağı

Bu, temiz bir Windows makinesindeki **en yaygın hata**dır. Rust'ın MSVC hedefi, MSVC linker'ı **`link.exe`**'ye ihtiyaç duyar; bu rustup ile GELMEZ, C++ build araçlarıyla gelir. Eksikse `cargo build` link aşamasında şu hatayı verir:

```
error: linker `link.exe` not found
```

**C++ build araçları kurulu mu, hızlı kontrol:**

```powershell
& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
```

Çıktı **boş değilse** C++ workload kuruludur — **2.4'e (build hızlı testi)** geç. Boşsa (veya `vswhere.exe` yoksa), aşağıdan kur.

**Kesin çözüm — "Desktop development with C++" workload'ını kur:**

```powershell
# winget ile (Visual Studio Community + C++ araçları + Windows SDK kurar):
winget install --id Microsoft.VisualStudio.2022.Community --source winget --force --override "--add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --addProductLang En-us"
```

Alternatifler:
- Zaten "**Desktop development with C++**" workload'lı Visual Studio varsa, işin bitti.
- Manuel: <https://visualstudio.microsoft.com/visual-cpp-build-tools/> adresinden "**Microsoft C++ Build Tools**"u indir, yükleyicide "**Desktop development with C++**" workload'ını işaretle. (Küçük ayak izi istersen minimum: "MSVC v143 - VS 2022 C++ x64/x86 build tools (Latest)" + bir Windows 11 SDK, ör. 10.0.22621.0.)
- Lisanslı bir VS yoksa yukarıdaki komutta `Community` yerine `BuildTools` kullanabilirsin.

> **KRİTİK:** C++ araçlarını kurduktan sonra yine **YENİ bir terminal aç** (yeni kurulan `link.exe`'nin PATH'te görünmesi için).

### 2.4 Build hızlı testi (linker gerçekten erişilebilir mi)

```powershell
cargo new hello_link_test
cd hello_link_test
cargo build
cd ..
```

Başarılı bir link, `link.exe`'nin erişilebilir olduğunu kanıtlar. Burada `link.exe not found` alırsan 2.3'e geri dön.

---

## 3. Çalıştır

> **Zihinsel model — bu en önemli nokta:** `SendInput`'un hedef pencere parametresi **yoktur.** Enjeksiyon anında (başlatmadan ~2 sn sonra) **klavye odağı hangi penceredeyse** `hello` oraya gider. Eğer terminal hâlâ öndeyse, `hello` **terminale** yazılır, Notepad'e değil. Bu yüzden Notepad'i **enjeksiyondan ÖNCE** odaklamalısın. Program 2 sn'yi bir kez bekler, hemen enjekte eder ve çıkar — ikinci şans yoktur, odağı-bekleyen bir döngü yoktur.

> **KRİTİK — yükseltme (elevation) eşleşmesi:** Terminali ve Notepad'i **ikisini de yükseltilmemiş (normal, "Run as administrator" DEĞİL)** çalıştır. Notepad yükseltilmiş, testin değilse, UIPI enjeksiyonu **sessizce** engeller (bkz. Sorun giderme). Güvenli varsayılan: her ikisi de normal kullanıcı.

### 3.1 Basit yöntem (elle odaklama)

1. `keyboard-it` klasörüne gir:

   ```powershell
   cd C:\yol\keyboard-it
   ```
   (Doğru klasör: kökünde `[workspace]` içeren `Cargo.toml` + `crates\win-receiver\` olmalı.)

2. **Notepad'i aç** (yükseltilmemiş):

   ```powershell
   notepad
   ```

3. Testi başlat:

   ```powershell
   cargo run -p win-receiver
   ```
   **İlk çalıştırma derler** (windows crate + bağımlılıklar) — biraz sürebilir, bu normaldir. Program şunu yazacak: `2 saniye içinde hedef pencereye (ör. Notepad) tıklayın...`

4. **O 2 saniye içinde Notepad'e tıkla** ki odak Notepad'te olsun. Program `hello`'yu enjekte edip `bitti: ...` yazacak ve çıkacak.

### 3.2 Sağlam yöntem (odak yarışını ortadan kaldıran launcher — önerilir)

Terminal odağı geri kapabildiği için, en güvenilir yol: **önce** Notepad'i öne getir, testi **ayrık (detached)** başlat (böylece terminal odağı geri almaz), sonra oku. Aşağıdaki script'i `keyboard-it` klasöründen çalıştır. Kendisi ön planda olan bir PowerShell'den çalıştır (yani script'i başlattığın terminal önde olsun).

```powershell
# keyboard-it klasöründe çalıştır. Önce build et:
cargo build -p win-receiver
# exe burada oluşur: .\target\debug\win-receiver.exe

# Notepad'i aç ve öne getir:
$np = Start-Process notepad -PassThru
Start-Sleep -Milliseconds 800
(New-Object -ComObject WScript.Shell).AppActivate($np.Id) | Out-Null

# Testi AYRIK başlat (terminal odağı geri almasın), sonra Notepad'i tekrar öne al:
Start-Process ".\target\debug\win-receiver.exe"
Start-Sleep -Milliseconds 200
(New-Object -ComObject WScript.Shell).AppActivate($np.Id) | Out-Null

# 2 sn enjeksiyon penceresi + pay:
Start-Sleep -Seconds 3
```

> Eğer odak Notepad'e geçmeyi reddederse (ön plan kilidi): script'i tıkladığın terminalden çalıştırdığından emin ol (son girdiyi o aldı sayılır), ya da bir kereliğine `HKCU\Control Panel\Desktop\ForegroundLockTimeout` değerini `0` yap, ya da script çalışırken Notepad penceresine fiziksel bir tıkla.

---

## 4. Başarı nasıl doğrulanır

**Başarı ölçütü:** Notepad'de tam olarak `hello` metni belirir.

Kanıtı **makine ile** yakala — Notepad'in metnini geri oku ve `hello`'ya eşit mi diye karşılaştır. UI Automation ile (modern/Win11 Notepad dahil çalışır), 3.2'deki `Start-Sleep -Seconds 3`'ten SONRA:

```powershell
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty, "Notepad")
$npWin = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)

$editCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Document)
$edit = $npWin.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $editCond)
if (-not $edit) {
    $editCond2 = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Edit)
    $edit = $npWin.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $editCond2)
}
$vp = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
$text = $vp.Current.Value

Write-Host "Notepad icerigi: [$text]"
if ($text.Trim() -eq "hello") { Write-Host "SONUC: PASS" } else { Write-Host "SONUC: FAIL" }
```

Alternatif (klasik Notepad, "Edit" sınıfı çocuk kontrol): `FindWindow('Notepad', ...)` + `FindWindowEx(..., 'Edit', ...)` + `WM_GETTEXT`.

**Ek kanıt — insan için ekran görüntüsü al:**

```powershell
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Drawing.Rectangle]::FromLTRB(0,0,[System.Windows.Forms.SystemInformation]::VirtualScreen.Width,[System.Windows.Forms.SystemInformation]::VirtualScreen.Height)
$bmp = New-Object System.Drawing.Bitmap($b.Width,$b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save("$PWD\m0-proof.png")
Write-Host "Ekran goruntusu: $PWD\m0-proof.png"
```

**Kontrolü açıkça yap:** metin `hello` mi (PASS) yoksa değil mi (FAIL)? `hello` terminalde çıktıysa bu **FAIL**'dir (odak sorunu — bkz. Sorun giderme).

---

## 5. Sorun giderme

| Belirti | Sebep | Çözüm |
|---|---|---|
| `error: linker ` `link.exe` ` not found` (build sırasında) | MSVC C++ araç zinciri yok | 2.3'ü yap: "Desktop development with C++" workload'ını kur, **yeni terminal aç**. |
| `hello` **terminale** yazıldı, Notepad'e değil | Enjeksiyon anında odak terminaldeydi (SendInput odaktaki pencereye yazar) | Notepad'i enjeksiyondan **önce** odakla; 3.2'deki sağlam launcher'ı kullan (ayrık başlat + Notepad'i öne al). |
| **Hiçbir şey olmadı** / `SendInput` 0 döndü (kısa sayım uyarısı) | Girdi başka bir thread'ce engellenmiş: düşük seviyeli klavye hook, kilitli/güvenli masaüstü, aktif UAC istemi, ekran koruyucu ya da kilitli oturum | Ekranda UAC/kilit/güvenli masaüstü olmadığından emin ol; tekrar dene. Notepad ve testin **ikisi de normal (yükseltilmemiş)** çalışsın. |
| **Hiçbir şey olmadı** ama program `bitti` dedi ve sayım uyarısı YOK | **UIPI (sessiz).** Notepad yükseltilmiş, testin değil (veya tersi). SendInput 1 (başarı) döner ama tuş yüksek bütünlük seviyeli hedefte düşürülür — dönüş değeri ve GetLastError bunu **göstermez** | İkisini de **aynı bütünlük seviyesinde** çalıştır. Güvenli varsayılan: **her ikisi de yükseltilmemiş**. Terminali "Run as administrator" ile açma. |
| Yalnızca **kısmi** metin çıktı (ör. `hel`) | Enjeksiyon sırasında odak kaydı, ya da pencere odağı ortada değişti | Odağı sabit tut; 3.2 launcher'ı + `Start-Sleep` payını kullan; tekrar çalıştır. |
| Build **hatası: eski 3-argümanlı `SendInput`** hakkında (imza uyuşmazlığı) | Yanlış `windows` crate sürümü. 0.62'de `SendInput` **slice tabanlıdır** (`&[INPUT]`, `i32`) | `crates/win-receiver/Cargo.toml`'da `windows = "0.62.2"` ve feature'ların `Win32_UI_Input_KeyboardAndMouse` + `Win32_Foundation` olduğunu doğrula. Sonra `cargo update -p windows` / `cargo clean` + yeniden derle. |
| `Access is denied. (os error 5)` veya rastgele dosya kilidi (build sırasında) | Antivirüs (Defender) `target\` klasörünü tararken kilitliyor | Genelde tekrar denemek işe yarar. Kalıcıysa Defender'da proje/`target` klasörü ve `%USERPROFILE%\.cargo` için istisna ekle. |
| `path too long` / derin bağımlılık yolu hataları | Eski 260 karakter MAX_PATH sınırı | Projeyi disk köküne yakın tut (ör. `C:\keyboard-it`), gerekirse bir kereliğine `HKLM\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled=1` (yönetici). |
| `cargo`/`rustc` "not found" (kurulumun hemen ardından) | Açık terminal yeni PATH'i görmüyor | **Yeni bir terminal aç** (VS Code kullanıyorsan onu da yeniden başlat). |

> **Not:** Bu test normalde **Yönetici gerektirmez.** Yerel olarak derlenmiş, imzasız bir cargo konsol binary'si genelde SmartScreen'e takılmaz (SmartScreen indirilen dosyaları hedefler). Yine de `hello` çıkmadıysa, exe'nin Defender tarafından karantinaya alınmadığını teyit et.

---

## 6. Bana ne rapor et

Aşağıdakilerin **hepsini** geri gönder:

1. **Tam `cargo` çıktısı** — başarı ya da hata, olduğu gibi (kes yapıştır). Derleme + çalıştırma çıktısı.
2. **`hello` belirdi mi?** Net `evet` / `hayır` + **kanıt**:
   - Bölüm 4'teki readback sonucu (`Notepad icerigi: [...]` ve `SONUC: PASS/FAIL`), ve/veya
   - `m0-proof.png` ekran görüntüsünün var olduğunu belirt.
   - `hello` **terminalde** çıktıysa bunu açıkça yaz (bu odak-FAIL demektir).
3. **Windows sürümü** — `cmd /c ver` veya `[System.Environment]::OSVersion.VersionString` çıktısı.
4. **Rust / araç zinciri sürümü** — `rustc --version`, `cargo --version` ve `rustup show` çıktısı (özellikle default host `...-msvc` mi `...-gnu` mi).
5. **Herhangi bir hata varsa: TAM hata metnini harfi harfine** ver (özetleme, paraphrase etme). Özellikle `link.exe not found`, os error 5, ya da SendInput sayım uyarısı gibi satırların birebir metni kritik.

> Kısa özet formatı (üstte), ardından ham çıktı blokları (altta) ideal. Başarısızsa **hata metnini kelimesi kelimesine** vermen, sorunu uzaktan teşhis edebilmemiz için en önemli tek şeydir.
