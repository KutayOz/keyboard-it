# keyboard-it

Control paired devices from your MacBook or Android phone/tablet over the local network, with full Windows desktop control and native keyboard/mouse hosts for macOS and Linux X11. Android devices can be added through ADB and opened in a managed scrcpy window.

keyboard-it saves multiple devices, pairs with each once, and reconnects to the selected
device automatically. Open **Cihazlarım…** in the Mac menu bar to add, select or forget a PC
and choose its local screen/input/clipboard permissions. Choose **Ekranı Aç** to open its existing desktop in a native
macOS full-screen Space. Switching to another Space returns keyboard and mouse control to
the Mac immediately. Text clipboard sharing works in both directions while the Windows
viewer is focused and can be disabled in the menu.

The Windows receiver captures the primary monitor with Desktop Duplication, encodes H.264
using Media Foundation (hardware first, software fallback), and sends it over a separately
authenticated, encrypted Noise channel. macOS decodes with VideoToolbox. No SSH, RDP account,
external remote-desktop application, or cloud service is required.

The original keyboard/mouse-only workflow remains available as **Yalnız klavye/fare modu**:
double-tap Fn to switch input between computers. Existing pairings and the legacy input
protocol remain compatible. Screen sharing requires the updated app on **both** computers.

**Windows desktop requirements:** same LAN, an awake Windows 10/11 PC with a signed-in user and an active
connected monitor. This version does not include internet access, headless displays, audio,
file transfer, or control of the Windows sign-in/UAC secure desktop. Screen streaming
resumes when the normal desktop becomes available again.

See [Remote desktop setup, architecture and validation](docs/remote-desktop.md).
See [Android phone/tablet controller APK and setup](docs/android-controller.md).
See [Mac → Android USB/Wi-Fi setup and validation limits](docs/android.md).
See [Universal devices, access management and validation without physical devices](docs/universal-devices.md).

## How it works

```
Mac (mac-sender)                                          Windows (win-receiver)
CGEventTap ──► HID usage codes ──► Noise NNpsk0 / TCP ──► scancodes ──► SendInput
capture + Fn toggle                encrypted, LAN only                  focused app
```

Pairing runs once, on its own port, before any of that exists:

```
Mac                                                       Windows
mDNS browse ──► picks a PC ──► Noise NN (unauthenticated) ──► "Allow this Mac?"
                                                                 │
                                                                 ▼
                                            ◄──── pairing key ── you click Allow
```

- Double-tap Fn toggles forwarding. While active, input is suppressed on the Mac and its
  cursor is frozen; keys, mouse movement, clicks, and scroll go to Windows. Double-tap Fn
  again to switch back.
- The current state is always visible: a menu bar item on the Mac, a tray icon on Windows.
- Cmd is mapped to Ctrl so shortcuts like copy/paste keep working; Turkish Q and F-keys are
  translated.
- The sender reconnects automatically, and the receiver releases held keys when the
  connection drops, so nothing stays stuck on the Windows side.

Workspace crates: `crates/protocol` (wire format, registry and access policy),
`crates/device-host` (shared host runtime, native input adapters and console host),
`crates/android-adapter` (explicit ADB discovery/pairing and managed scrcpy sessions),
`crates/mac-sender` (macOS controller), and `crates/win-receiver` (Windows tray and settings).

## Install

Download page: https://kutayoz.github.io/keyboard-it/ — or fetch the installers directly:

- macOS: https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-macos.dmg
- Windows: https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-windows-x64.msi

The binaries are unsigned (see [Security model](#security-model)), so both OSes warn on first
launch.

### macOS

Alternatively, the terminal installer checks the pinned release's SHA-256 digest before
mounting the image and installing the app:

```sh
curl -fsSL https://kutayoz.github.io/keyboard-it/install-macos.sh | sh
```

The script downloads the DMG and checksum manifest from the same GitHub Release, verifies
it inside a private temporary directory, mounts it, copies `keyboard-it.app` to `/Applications`,
and opens it. A missing/mismatched checksum stops installation. Files fetched by curl do not
inherit browser quarantine; neither this behavior nor a checksum verifies publisher identity.

If you install from the `.dmg` instead, macOS blocks the unsigned app on first open:

- macOS 15 (Sequoia): open the app, dismiss the "Apple could not verify" dialog, then go to
  System Settings → Privacy & Security → scroll to the "keyboard-it was blocked" row →
  **Open Anyway** → open the app again.
- macOS 14 and earlier: right-click the app in Applications → Open → Open.

On first launch a **Permissions** window opens and walks through everything macOS asks for,
in order, showing which ones are done as you go:

1. **Input Monitoring** — lets the app see keystrokes so it can forward them.
2. **Accessibility** — lets it hold those keystrokes back from the Mac while you type on Windows.
3. **Local Network** — lets it find your PC, so you never type an IP address. macOS exposes
   no API for reading this one, so the window can only confirm it once something on the
   network answers (or once a session with the PC is up). If it stays unconfirmed, nothing
   is necessarily wrong — carry on and pair.

Each system prompt appears on its own as the previous permission lands; the window ends with
a single **Restart** (macOS only applies these to a freshly launched process). Reopen it any
time from the menu bar.

#### If macOS never asks, or keyboard-it looks switched on but still cannot capture

This is the cost of an unsigned build, not a bug in the grant. macOS binds Input Monitoring
and Accessibility to a *designated requirement*, and with an ad-hoc signature that requirement
is the binary's own code hash — so every new build is a different app to macOS. The old row
survives in System Settings while the approval behind it does not, and because macOS keeps
exactly one saved answer per bundle identifier, it never prompts again.

The permissions window detects both dead ends and says so. The cure either way:

- In System Settings → Privacy & Security → the relevant list: select keyboard-it, remove it
  with **−**, add it back with **+**, then restart the app. Or
- run the command the window's **Copy reset command** button puts on your clipboard:
  `sudo tccutil reset All com.keyboard-it.keyboard-it` (it needs root — these records live in
  the system TCC database, which is why the app cannot clear them itself), then relaunch.

`packaging/mac/package.sh` now pins the requirement to the bundle identifier instead
(`codesign -r='designated => identifier "com.keyboard-it.keyboard-it"'`), so builds from
this repo keep their permissions from one version to the next. You can see what any build
asks for:

```bash
codesign -d -r- /Applications/keyboard-it.app
```

`identifier "…"` is good; `cdhash H"…"` is a build that will lose its permissions on the
next update. Note the trade-off: an identifier-only requirement would also be satisfied by
any other binary claiming that bundle identifier. Signing with a certificate is stricter —
the requirement is then both stable *and* scoped to that certificate:

1. Keychain Access → Certificate Assistant → **Create a Certificate…** → name `keyboard-it`,
   type **Code Signing**, self-signed.
2. Build with it:

```bash
KEYBOARD_IT_SIGN_ID=keyboard-it packaging/mac/package.sh
```

One-time note when moving off an old build: the permission macOS already saved still carries
the *old* requirement, so it has to be cleared once with `sudo tccutil reset All
com.keyboard-it.keyboard-it` before the new one can be granted.

One thing the app cannot set for you: System Settings → Keyboard → "Press fn key to" →
**Do Nothing**. Otherwise macOS grabs double-Fn for Dictation or the emoji picker and the
toggle misfires.

The app lives in the menu bar; its Dock icon appears while the Windows viewer is open: **Permissions** reopens the window above,
**Cihazlarım…** opens saved devices and pairing, **Start at Login** toggles a LaunchAgent, and
**Quit** exits and restores normal cursor behavior.

### Windows

Run the `.msi`. SmartScreen flags the unsigned installer: click **More info → Run anyway**.
The receiver runs in the system tray and starts listening on its own — there is nothing to
configure before pairing. The installer adds its own inbound firewall rule, so no Defender
prompt should appear; if you installed by copying the `.exe` instead, answer **Allow** to
the prompt on first launch. Its settings window sets the port, toggles start-at-login, and
lists authorized controllers and can revoke one controller or all access.

### Pairing

Nothing is typed and no addresses are exchanged. With keyboard-it running on both machines
and both on the same network:

1. On the Mac, open **Cihazlarım…** from the menu bar. Your PC appears in the discovery list within a
   few seconds.
2. Pick it and click **Pair & Connect**.
3. Compare **all eight groups of the fingerprint** on both computers. On the PC, approve **Allow** only when every group matches.

That's it — the Mac stores the key the PC hands over and connects within about a second.

Comparing the complete fingerprint and approving the request is the security check: whoever you
allow can type and click on your PC for as long as the pairing lasts. Allow a machine you
recognise, on a network you trust — see [Security model](#security-model) for what that
does and does not cover.

Other things worth knowing:

- Each approved controller receives its own random key, persisted before pairing succeeds; you never copy it.
- The Mac stores the PC's `.local` name, so the link survives the PC getting a new IP.
- **Erişimi kaldır** on Windows revokes one controller and closes its sessions.
  **Tüm erişimleri kaldır** revokes all access. Earlier pairings that shared one key are
  shown as a legacy group; re-pair each controller to get separate revocation.
  **Cihazı Unut** on the Mac forgets only the selected device locally.
- The PC only accepts one pairing request at a time, declines on its own after two
  minutes, and ignores repeat requests for a few seconds after you decline one. The Mac
  counts that deadline down on screen, so you know how long you have to walk over.
- Discovery uses mDNS (the same mechanism as AirPlay and printer discovery). On a network
  that blocks multicast, or with client isolation on, the PC will not show up. The TCP
  port defaults to `5599`; the pairing listener uses `5600`, or an ephemeral port if that
  one is taken — which is why the installer's firewall rule covers the *program* rather
  than a fixed port.

## Android phone/tablet → Windows or Mac

Install [keyboard-it-android.apk](https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-android.apk)
on Android 8+ ARM64/x86_64. Windows supports screen and input; the Mac app's new
**Android’den bu Mac’i kontrol et…** mode supports keyboard/mouse only (no Mac screen).
New authenticated Unicode input requires an updated 0.7.0 host. Pause another controller
before connecting to the same PC. Compare all eight fingerprint groups on both devices.
Credentials use Android Keystore AES-GCM storage; backgrounding closes the session.
See [setup, security, build and exact validation limits](docs/android-controller.md).

## Mac → Android

Open **Cihazlarım… → Android Ekle…** on the Mac. With ADB and scrcpy installed, refresh
the list, approve USB debugging on the phone, select a ready device and save it. Then
select it with **Bu Cihaza Geç** and choose **Ekranı Aç**. The setup window also accepts
Android wireless debugging's separate pairing address/code and connection address.

The Android viewer is a separate managed scrcpy window. Input can be disabled for
view-only use; audio and automatic clipboard sync are disabled. Target/permission
changes and quitting close the owned viewer. Existing computer pairings remain intact.
USB discovery, saving/selecting a phone, live video and mouse control were validated on
an SM-G780G running Android 13. Wi-Fi pairing and keyboard behavior still need physical
validation. See [Android setup and limits](docs/android.md).

## Build from source

Requires a Rust toolchain (https://rustup.rs).

```sh
cargo build --release        # all crates for the host OS
cargo run -p mac-sender      # macOS side
cargo run -p win-receiver    # Windows side
```

On a non-Windows host, `win-receiver` prints received events instead of injecting them and
refuses new pairing requests because it has no consent UI. Use the console host below
for a locally approved, device-free pairing test. Both binaries resolve to the same config path, so give
them separate files:

```sh
KEYBOARD_IT_CONFIG=/tmp/kbit-receiver.toml cargo run -p win-receiver
```


### macOS and Linux input hosts

The shared host can also receive keyboard/mouse events on another Mac or Linux X11
computer. These adapters currently support **input only**; their screen and clipboard
capabilities are disabled. Native injection has been compiled and tested through
simulations; physical device validation remains outstanding.

```sh
cargo run -p device-host --bin keyboard-it-host -- --help
cargo run -p device-host --bin keyboard-it-host
# Device-free simulation: no OS input, local pairing consent still required
cargo run -p device-host --bin keyboard-it-host -- --dry-run --config /tmp/kbit-host.toml
```

macOS requires Accessibility permission for the host executable. Linux requires an
active X11 session, libX11, libXtst and XTEST; Wayland is explicitly unsupported.
The console host uses its own `host.toml` by default, so it can run alongside the Mac
controller. Pair it from **Cihazlarım…**, compare both codes, then enter `allow CODE`
in the host console. Enable **Yalnız klavye/fare modu** on the controller and use double Fn.

Console commands: `list`, `revoke ID`, `revoke-all`, `deny`, and `quit`. EOF also stops
listeners and releases held input. With the host stopped, `--list-controllers`,
`--revoke ID` and `--revoke-all` manage access offline. Use `--config PATH` for a custom
config. The console host is built from source; the current DMG/MSI packaging is unchanged.

Packaging:

- macOS `.dmg`: `packaging/mac/package.sh` builds a release binary, wraps it into
  `keyboard-it.app` (menu bar agent, ad-hoc signed), and produces
  `dist/keyboard-it-<version>.dmg`. Needs the Xcode command line tools; Python 3 with Pillow
  only if you regenerate the icon (`packaging/mac/make_icon.py`).
- Windows `.msi`: `cargo install cargo-wix`, then `cargo wix --package win-receiver`
  (WiX v3).
- CI (`.github/workflows/build.yml`) builds both installers on every `v*` tag and attaches
  them to a GitHub Release, including fixed-name copies for the `latest/download` links.

## Security model

Built for a trusted home or office LAN, not the open internet.

**Session (port 5599).** Transport is `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s` (the `snow`
crate). Both sides prove knowledge of a pre-shared key derived from `shared_secret` with
BLAKE2s; all traffic is encrypted with per-session ephemeral keys. Sessions require an authorized key; wrong
keys cannot connect. The receiver listens on all IPv4 interfaces. The Windows installer limits its inbound
rule to the local subnet; the Noise key authenticates peers within that boundary. Legacy
input ownership transfers only after a valid fresh-session transport record, not merely
a replayable initial handshake message.

**Pairing (port 5600).** This is where the key comes from, so it cannot use the key. It
runs `Noise_NN` — ephemeral keys only, no PSK — which is encrypted but *unauthenticated*.
The gate is consent, not cryptography: no Mac gets a key unless someone at the PC clicks
**Allow**.

Both screens show a full 256-bit fingerprint derived from the Noise handshake hash.
Compare **all eight groups** before allowing the request. An active intermediary establishes
separate handshakes; matching their complete fingerprints requires a cryptographic collision.
Approving without comparison still permits MITM. Pairing v2 refuses the old six-digit scheme;
update both apps to 0.6.2 for new pairings. Existing saved session keys remain usable.
See [Security review and regression evidence](docs/security-review.md).

The receiver limits abuse of that dialog: one request at a time, a 10-second I/O timeout, a
two-minute auto-decline, and a cooldown after a decline. That deadline lives in
`protocol::PAIR_DECISION_TIMEOUT` rather than on either side, because both machines make
promises about it to two different people — when it was defined separately the PC gave up
at 60 s while the Mac promised nothing, and a click that arrived at 60.6 s was reported as
"approved" on the PC and as a rejection on the Mac.

**Remote desktop (default port 5601).** A separate Noise NNpsk0 channel derives a
domain-separated key from that controller’s pairing key and negotiates its own version and device
identity. Updated peers exchange authenticated screen/input/clipboard and codec capabilities;
older peers retain the original exchange. Desktop data, input and text clipboard are encrypted. A new connection replaces
an old session only after authentication. Video and clipboard reassembly are bounded.

**Everywhere.**

- Controller keys remain plaintext: the sender config and receiver `*.access.toml` file
  use mode 0600 on Unix. Anyone who can read them can impersonate an authorized peer.
- New pairings have individual credentials. Revocation persists before taking effect,
  closes only that controller’s sockets, and rejects stale authenticated handshakes.
  The earlier shared key remains a separately revocable legacy group until re-pairing.
- The receiver locks its access policy for its lifetime. A second process refuses to
  edit it; manage live access in the running Windows settings or host console.
- Pairing is offered whenever the receiver is listening. Anyone on your LAN can make the
  dialog appear — they just cannot get past it without someone at the PC clicking Allow.
- Device names shown in the dialog come from the network. They are stripped of control
  characters and length-capped before display, but they are still self-reported: a name is
  a label, not an identity. Recognise the machine before you allow it.
- Binaries are unsigned and not notarized — hence the Gatekeeper and SmartScreen warnings.

## License

MIT — see [LICENSE](LICENSE).

### 0.5.0 görüntü motoru

GPU üzerinde imleç/döndürme/ölçekleme ve NV12 dönüşümü, D3D11 yüzeylerinden donanım kodlama, 60/120 FPS profilleri ve Mac ekranına bağlı görüntü yenilemesi eklendi. Menüdeki **Görüntü kalitesi** ve **Performans bilgilerini göster** seçeneklerini kullanın. Masaüstü için her iki cihazın da 0.5.0 olması gerekir; eşleştirme korunur. Ayrıntılar ve ölçüm sınırları: [Remote desktop](docs/remote-desktop.md).

### 0.6.2 device management and security

Adds Android ADB/scrcpy integration, macOS/Linux X11 input hosts and individual controller
revocation. Pairing now requires comparing the complete fingerprint (all eight groups),
and both apps must be 0.6.2 for new pairings. Existing sessions retain their saved keys.
Fixes a captured-handshake replay that could disconnect the active input controller,
restricts the Windows firewall exception to the local subnet, and removes diagnostic
auto-approval. See [security evidence and limits](docs/security-review.md).

### 0.7.0 Android controller

Adds an installable Android controller APK using the shared Rust Noise protocol, Windows
H.264 viewing and touch control, an explicit Mac input-host mode with local pairing consent
and revocation, and negotiated Unicode text input. Mac screen sharing remains unsupported.
The Android and native tests are distinguished from physical phone/tablet validation in
[Android controller documentation](docs/android-controller.md).
