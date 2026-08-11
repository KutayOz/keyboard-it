# keyboard-it

Use a MacBook's keyboard and trackpad to control a Windows PC over the local network.

keyboard-it is a small software KVM: a menu bar app on the Mac captures keyboard and mouse
input and streams it, encrypted, to a tray app on the Windows machine, which injects it into
whatever window has focus. No extra hardware, no cloud — one TCP connection on your LAN.
Double-tap the Fn key to switch input between the two machines.

Setup is two clicks and nothing to type: the Mac finds the PC on the network, you pick it,
and the PC asks whether to allow it.

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

The app lives in the menu bar (no Dock icon): **Permissions** reopens the window above,
**Settings** opens the settings window, **Start at Login** toggles a LaunchAgent, and
**Quit** exits and restores normal cursor behavior.

### Windows

Run the `.msi`. SmartScreen flags the unsigned installer: click **More info → Run anyway**.
The receiver runs in the system tray and starts listening on its own — there is nothing to
configure before pairing. The installer adds its own inbound firewall rule, so no Defender
prompt should appear; if you installed by copying the `.exe` instead, answer **Allow** to
the prompt on first launch. Its settings window sets the port, toggles start-at-login, and
can forget paired Macs.

### Pairing

Nothing is typed and no addresses are exchanged. With keyboard-it running on both machines
and both on the same network:

1. On the Mac, open **Settings** from the menu bar. Your PC appears in the list within a
   few seconds.
2. Pick it and click **Pair & Connect**.
3. On the PC, a window asks *"Allow this Mac to control this PC?"* Click **Allow**.

That's it — the Mac stores the key the PC hands over and connects within about a second.

Your click is the whole security check, so it is worth a second of attention: whoever you
allow can type and click on your PC for as long as the pairing lasts. Allow a machine you
recognise, on a network you trust — see [Security model](#security-model) for what that
does and does not cover.

Other things worth knowing:

- The PC generates its key by itself on first run; you never see or copy it.
- The Mac stores the PC's `.local` name, so the link survives the PC getting a new IP.
- **Forget paired Macs** in the Windows settings window issues a new key, which locks out
  every Mac paired so far. **Unpair** on the Mac just forgets the PC locally.
- The PC only accepts one pairing request at a time, declines on its own after two
  minutes, and ignores repeat requests for a few seconds after you decline one. The Mac
  counts that deadline down on screen, so you know how long you have to walk over.
- Discovery uses mDNS (the same mechanism as AirPlay and printer discovery). On a network
  that blocks multicast, or with client isolation on, the PC will not show up. The TCP
  port defaults to `5599`; the pairing listener uses `5600`, or an ephemeral port if that
  one is taken — which is why the installer's firewall rule covers the *program* rather
  than a fixed port.

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
The gate is consent, not cryptography: no Mac gets a key unless someone at the PC clicks
**Allow**.

One gap is left open deliberately. Both sides can derive a short code from the handshake
hash, which commits to both ephemeral public keys — an active attacker has to run two
separate handshakes and so cannot make both codes match. That code used to be shown on both
screens for the user to compare. It is not any more, because a code nobody compares
protects nobody, and nobody compared it. What that costs: someone able to intercept traffic
on your LAN during the few seconds a pairing is in flight can sit in the middle and come
away holding the key. Pair on a network you trust. (`protocol::pairing` still derives the
code and never puts it on the wire, so bringing it back is a UI change, not a protocol one.)

The receiver limits abuse of that dialog: one request at a time, a 10-second I/O timeout, a
two-minute auto-decline, and a cooldown after a decline. That deadline lives in
`protocol::PAIR_DECISION_TIMEOUT` rather than on either side, because both machines make
promises about it to two different people — when it was defined separately the PC gave up
at 60 s while the Mac promised nothing, and a click that arrived at 60.6 s was reported as
"approved" on the PC and as a rejection on the Mac.

**Everywhere.**

- The pairing key is stored in plaintext in the local config file (mode 0600 on macOS).
  Anyone who can read that file can impersonate a peer.
- One key per PC, shared by every Mac paired to it. "Forget paired Macs" issues a new one
  and locks all of them out; there is no per-device revocation.
- Pairing is offered whenever the receiver is listening. Anyone on your LAN can make the
  dialog appear — they just cannot get past it without someone at the PC clicking Allow.
- Device names shown in the dialog come from the network. They are stripped of control
  characters and length-capped before display, but they are still self-reported: a name is
  a label, not an identity. Recognise the machine before you allow it.
- Binaries are unsigned and not notarized — hence the Gatekeeper and SmartScreen warnings.

## License

MIT — see [LICENSE](LICENSE).
