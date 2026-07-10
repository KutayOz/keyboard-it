# keyboard-it

Use a MacBook's keyboard and trackpad to control a Windows PC over the local network.

keyboard-it is a small software KVM: a menu bar app on the Mac captures keyboard and mouse
input and streams it, encrypted, to a tray app on the Windows machine, which injects it into
whatever window has focus. No extra hardware, no Bluetooth pairing, no cloud — one TCP
connection on your LAN. Double-tap the Fn key to switch input between the two machines.

## How it works

```
Mac (mac-sender)                                          Windows (win-receiver)
CGEventTap ──► HID usage codes ──► Noise NNpsk0 / TCP ──► scancodes ──► SendInput
capture + Fn toggle                encrypted, LAN only                  focused app
```

- Double-tap Fn toggles forwarding. While active, input is suppressed on the Mac and its
  cursor is frozen; keys, mouse movement, clicks, and scroll go to Windows. Double-tap Fn
  again to switch back.
- The current state is always visible: a menu bar item on the Mac, a tray icon on Windows.
- Cmd is mapped to Ctrl so shortcuts like copy/paste keep working; Turkish Q and F-keys are
  translated.
- The sender reconnects automatically, and the receiver releases held keys when the
  connection drops, so nothing stays stuck on the Windows side.

Workspace crates: `crates/protocol` (wire format, config, Noise handshake),
`crates/mac-sender` (macOS menu bar app), `crates/win-receiver` (Windows tray app with a
settings window).

## Install

Download page: https://kutayoz.github.io/keyboard-it/ — or fetch the installers directly:

- macOS: https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-macos.dmg
- Windows: https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-windows-x64.msi

The binaries are unsigned (see [Security model](#security-model)), so both OSes warn on first
launch.

### macOS

Terminal install skips the Gatekeeper prompt entirely, because files downloaded with curl
carry no quarantine flag:

```sh
curl -fsSL https://kutayoz.github.io/keyboard-it/install-macos.sh | sh
```

The script downloads the DMG from GitHub Releases, mounts it, copies `keyboard-it.app` to
`/Applications`, and opens it.

If you install from the `.dmg` instead, macOS blocks the unsigned app on first open:

- macOS 15 (Sequoia): open the app, dismiss the "Apple could not verify" dialog, then go to
  System Settings → Privacy & Security → scroll to the "keyboard-it was blocked" row →
  **Open Anyway** → open the app again.
- macOS 14 and earlier: right-click the app in Applications → Open → Open.

Required settings — the app cannot capture input without them:

1. System Settings → Privacy & Security → **Input Monitoring** → enable keyboard-it.
2. System Settings → Privacy & Security → **Accessibility** → enable keyboard-it.
3. System Settings → Keyboard → "Press fn key to" → **Do Nothing**. Otherwise macOS grabs
   double-Fn for Dictation or the emoji picker and the toggle misfires.

Quit and reopen the app after granting permissions; they only apply to a freshly launched
process. Permissions are tied to the binary's path, so grant them again if you move the
`.app`.

The app lives in the menu bar (no Dock icon) with three entries: **Settings** opens the
config file in a text editor, **Start at Login** toggles a LaunchAgent, and **Quit** exits
and restores normal cursor behavior.

### Windows

Run the `.msi`. SmartScreen flags the unsigned installer: click **More info → Run anyway**.
The receiver runs in the system tray; its settings window takes the pairing key and port and
can enable start-at-login. Allow it through the Windows firewall when prompted — it listens
on the configured TCP port.

### Pairing

Both machines read a `config.toml` and must share the same pairing key. Generate one strong
random value (for example with `openssl rand -base64 24`) and enter it on both sides.

| Field           | Meaning                                                            |
|-----------------|--------------------------------------------------------------------|
| `shared_secret` | Pairing key. Must be identical on both machines.                   |
| `peer_host`     | LAN IP of the Windows PC. Sender only; the receiver only listens.  |
| `role`          | `sender` on the Mac, `receiver` on Windows.                        |
| `port`          | TCP port, default `5599`. Must match on both sides.                |

On the Mac, edit the file via the menu bar **Settings** entry; it lives at
`~/Library/Application Support/com.keyboard-it.keyboard-it/config.toml`. On Windows, use the
settings window from the tray icon. A missing key is fatal — the programs refuse to start —
and a mismatched key fails the handshake.

## Build from source

Requires a Rust toolchain (https://rustup.rs).

```sh
cargo build --release        # all crates for the host OS
cargo run -p mac-sender      # macOS side
cargo run -p win-receiver    # Windows side
```

On a non-Windows host, `win-receiver` prints received events instead of injecting them, so
the whole network path can be exercised on one machine against `127.0.0.1`.

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

- Transport is `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s` (the `snow` crate). Both sides prove
  knowledge of a pre-shared key derived from `shared_secret` with BLAKE2s; all traffic is
  encrypted with per-session ephemeral keys. No key, no start; wrong key, no connection.
- The pairing key is stored in plaintext in the local config file (mode 0600 on macOS).
  Anyone who can read that file can impersonate a peer.
- The receiver listens on all interfaces on the configured port; the pre-shared key is the
  only gate.
- Binaries are unsigned and not notarized — hence the Gatekeeper and SmartScreen warnings.

## License

MIT — see [LICENSE](LICENSE).
