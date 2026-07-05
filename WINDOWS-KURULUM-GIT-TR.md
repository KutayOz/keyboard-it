# Windows'ta Git ile Kodu Alma ve Güncelleme

Bu proje artık GitHub'da (**özel** repo). Windows tarafı klasörü elle kopyalamak yerine
`git` ile alır ve her güncellemede `git pull` yapar.

**Repo:** https://github.com/KutayOz/keyboard-it  (private)

---

## Ön koşul: Git kurulu mu?

```powershell
git --version
```

Yoksa kur (biri yeterli):

```powershell
winget install Git.Git
# veya: winget install GitHub.cli   (gh — clone'u kolaylaştırır)
```

> Kurulumdan sonra **yeni bir terminal** aç (PATH güncellensin).

---

## İlk kez: repoyu clone et

Özel repo olduğu için **bir kereye mahsus GitHub kimlik doğrulaması** gerekir. İki yol:

**Yol A — `gh` ile (en kolay):**
```powershell
gh auth login          # tarayıcıda KutayOz hesabıyla giriş yap (bir kez)
gh repo clone KutayOz/keyboard-it
cd keyboard-it
```

**Yol B — düz `git` ile:**
```powershell
git clone https://github.com/KutayOz/keyboard-it.git
cd keyboard-it
```
İlk clone'da **Git Credential Manager** tarayıcıda bir GitHub giriş penceresi açar —
**KutayOz** hesabıyla giriş yap. (Pencere çıkmazsa: bir Personal Access Token gerekir;
kullanıcıdan iste.)

---

## Güncelleme (Mac'te her değişiklikten sonra)

Mac'te kod değişip push'landığında, Windows'ta sadece:

```powershell
cd C:\yol\keyboard-it
git pull
```

Sonra ilgili milestone'u çalıştır (ör. `cargo run -p win-receiver`).

> `git pull` "Already up to date" diyorsa, Mac tarafında henüz push yapılmamış demektir —
> kullanıcıya sor.

---

## Sık sorun

| Belirti | Çözüm |
|---|---|
| `git pull` "Authentication failed" | `gh auth login` (Yol A) ya da PAT ile yeniden dene. |
| `Permission denied` / repo görünmüyor | Özel repo; doğru hesapla (KutayOz) giriş yaptığından emin ol. |
| Yerel değişiklikler pull'u engelliyor | Windows'ta kod DÜZENLEME yapma; gerekiyorsa `git stash` veya `git reset --hard origin/main` (dikkat: yerel değişiklikleri siler). |
