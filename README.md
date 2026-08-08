# keyboard-it

Use a MacBook's keyboard and trackpad to control a Windows PC over the local network.

keyboard-it is a small software KVM: a menu bar app on the Mac captures keyboard and mouse
input and streams it, encrypted, to a tray app on the Windows machine, which injects it into
whatever window has focus. No extra hardware, no cloud — one TCP connection on your LAN.
Double-tap the Fn key to switch input between the two machines.

Setup is two clicks and nothing to type: the Mac finds the PC on the network, you pick it,
and the PC asks you to confirm a 6-digit code.

## How it works

```
Mac (mac-sender)                                          Windows (win-receiver)
CGEventTap ──► HID usage codes ──► Noise NNpsk0 / TCP ──► scancodes ──► SendInput
capture + Fn toggle                encrypted, LAN only                  focused app
```

Pairing runs once, on its own port, before any of that exists:

```
Mac                                                       Windows
mDNS browse ──► picks a PC ──► Noise NN (unauthenticated) ──► "Allow? code 482 913"
                               6-digit code from the             │
                               handshake hash, shown             ▼
                               on BOTH screens ◄──── pairing key ── you click Allow
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

On first launch the app walks you through the permissions it needs — the app cannot capture
input without them:

1. System Settings → Privacy & Security → **Input Monitoring** → enable keyboard-it.
2. System Settings → Privacy & Security → **Accessibility** → enable keyboard-it.
3. System Settings → Keyboard → "Press fn key to" → **Do Nothing**. Otherwise macOS grabs
   double-Fn for Dictation or the emoji picker and the toggle misfires.

The app relaunches itself once permissions are granted (they only apply to a freshly
launched process). Permissions are tied to the binary's path, so grant them again if you
move the `.app`.

The app lives in the menu bar (no Dock icon) with three entries: **Settings** opens the
settings window, **Start at Login** toggles a LaunchAgent, and **Quit** exits and restores
normal cursor behavior.

### Windows

Run the `.msi`. SmartScreen flags the unsigned installer: click **More info → Run anyway**.
The receiver runs in the system tray and starts listening on its own — there is nothing to
configure before pairing. Allow it through the Windows firewall when prompted. Its settings
window sets the port, toggles start-at-login, and can forget paired Macs.

### Pairing

Nothing is typed and no addresses are exchanged. With keyboard-it running on both machines
and both on the same network:

1. On the Mac, open **Settings** from the menu bar. Your PC appears in the list within a
   few seconds.
2. Pick it and click **Pair & Connect**. A 6-digit code appears.
3. On the PC, a window asks *"Allow this Mac to control this PC?"* with the same code.
   Click **Allow**.

That's it — the Mac stores the key the PC hands over and connects within about a second.

The code is the security check, so glance at it: **allow only if both screens show the same
number.** The pairing channel is encrypted but not yet authenticated, and the code is
derived from the handshake, so anyone sitting in the middle would produce two different
numbers. Whoever you allow can type and click on your PC.

Other things worth knowing:

- The PC generates its key by itself on first run; you never see or copy it.
- The Mac stores the PC's `.local` name, so the link survives the PC getting a new IP.
- **Forget paired Macs** in the Windows settings window issues a new key, which locks out
  every Mac paired so far. **Unpair** on the Mac just forgets the PC locally.
- The PC only accepts one pairing request at a time, declines on its own after 60 seconds,
  and ignores repeat requests for a few seconds after you decline one.
- Discovery uses mDNS (the same mechanism as AirPlay and printer discovery). On a network
  that blocks multicast, or with client isolation on, the PC will not show up. The TCP
  port defaults to `5599`; the pairing listener uses `5600`. Allow keyboard-it through the
  Windows firewall when prompted.

## Build from source

Requires a Rust toolchain (https://rustup.rs).

```sh
cargo build --release        # all crates for the host OS
cargo run -p mac-sender      # macOS side
cargo run -p win-receiver    # Windows side
```

On a non-Windows host, `win-receiver` prints received events instead of injecting them and
auto-accepts pairing requests (there is no dialog to confirm in), so the whole network path
can be exercised on one machine. Both binaries resolve to the same config path, so give
them separate files:

```sh
KEYBOARD_IT_CONFIG=/tmp/kbit-receiver.toml cargo run -p win-receiver
```

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
BLAKE2s; all traffic is encrypted with per-session ephemeral keys. No key, no start; wrong
key, no connection. The receiver listens on all interfaces and the key is the only gate.

**Pairing (port 5600).** This is where the key comes from, so it cannot use the key. It
runs `Noise_NN` — ephemeral keys only, no PSK — which is encrypted but *unauthenticated*.
Authentication is the human: both sides derive a 6-digit code from the Noise handshake
hash, which commits to both ephemeral public keys, and show it. An active attacker has to
run two separate handshakes and so cannot make both codes match. Your click on **Allow**
both authorizes the pairing and confirms the channel — which is why the codes are worth
comparing rather than clicking through.

The receiver limits abuse of that dialog: one request at a time, a 10-second I/O timeout, a
60-second auto-decline, and a cooldown after a decline.

**Everywhere.**

- The pairing key is stored in plaintext in the local config file (mode 0600 on macOS).
  Anyone who can read that file can impersonate a peer.
- One key per PC, shared by every Mac paired to it. "Forget paired Macs" issues a new one
  and locks all of them out; there is no per-device revocation.
- Pairing is offered whenever the receiver is listening. Anyone on your LAN can make the
  dialog appear — they just cannot get past it without someone at the PC clicking Allow.
- Device names shown in the dialog come from the network. They are stripped of control
  characters and length-capped before display, but they are still self-reported: a name is
  a label, not an identity. The code is what you verify.
- Binaries are unsigned and not notarized — hence the Gatekeeper and SmartScreen warnings.

## License

MIT — see [LICENSE](LICENSE).
