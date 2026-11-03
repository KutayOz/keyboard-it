# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

keyboard-it: a macOS menu-bar controller that drives paired devices on the LAN — keyboard/mouse
forwarding (double-tap Fn), a full Windows remote desktop (H.264 over Noise), native input-only
hosts for macOS/Linux X11, Android targets through ADB + a managed scrcpy window, and a native Android controller APK.
Android → Windows has screen/input; Android → Mac uses the explicit input-only Mac host. Rust workspace,
single version in the root `Cargo.toml` `[workspace.package]` (all crates inherit it).

## Commands

```sh
cargo test --workspace --locked                      # full suite (~2 min cold on macOS)
cargo test -p protocol --test controller_access      # one integration test file
cargo test -p protocol --test device_registry NAME   # one test by name filter
cargo test -p mac-sender NAME                        # mac-sender unit tests live in the binary

# macOS-only native smoke checks (hidden windows, no network/ADB/input). DEBUG BUILDS ONLY —
# the flags are gated on debug_assertions in mac-sender/src/main.rs.
cargo run -p mac-sender --locked -- --check-devices
cargo run -p mac-sender --locked -- --check-viewer

# Type-check other OSes from macOS (both rustup targets are installed here)
cargo check -p win-receiver -p device-host --target x86_64-pc-windows-gnu --tests --locked
cargo check -p android-adapter -p protocol -p mac-sender -p device-host --target x86_64-unknown-linux-gnu --all-targets --locked
```

The X11 adapter test (`device-host --test x11_input`) is `#[ignore]` and only runs on Linux under
Xvfb (see `.github/workflows/build.yml`). Windows-native tests (Media Foundation encoder, WARP
compute shader) only execute on the Windows CI runner; locally they are only type-checked.

There is no rustfmt/clippy config and CI does not enforce either. The tree is **not**
rustfmt-clean, so never run `cargo fmt --all`; format only files you touched.

Running things locally:

- `cargo run -p mac-sender -- --devices` opens the device manager. mac-sender **writes its
  config on startup** and holds `127.0.0.1:5598` as a single-instance lock, so a dev run
  collides with an installed keyboard-it and touches the real config unless you set
  `KEYBOARD_IT_CONFIG=/path/scratch.toml`.
- `KEYBOARD_IT_CONFIG=/tmp/kbit-receiver.toml cargo run -p win-receiver` — on non-Windows this
  is a dry run: prints received events and refuses pairing without a consent UI (`device-host/src/serve.rs`).
- `cargo run -p device-host --bin keyboard-it-host -- --dry-run --config /tmp/kbit-host.toml` —
  console host; pairing always needs a local `allow CODE`, even in dry run.
- Packaging: `packaging/mac/package.sh` (→ `dist/*.dmg`, optional `KEYBOARD_IT_SIGN_ID`);
  Windows `.msi` via `cargo wix` with the firewall-extension flags shown in `build.yml`.
  Releases are cut by pushing a `v*` tag; `site/README.md` lists the manual site steps.
- `.claude/launch.json` has a `site` preview config for the static landing page in `site/`.

Other env vars: `KEYBOARD_IT_ADB` / `KEYBOARD_IT_SCRCPY` (absolute tool paths, no silent
fallback), `KEYBOARD_IT_KEY` (legacy PSK fallback), `KEYBOARD_IT_VIRTUAL_DISPLAY` (Xvfb test gate).

## Architecture

Crate roles (dependency direction: everything → `protocol`):

- `protocol` — everything both sides must agree on: wire formats, Noise transports, pairing,
  config/device registry, receiver access policy, capabilities, stream profiles. Shared
  constants that two UIs promise to a user (e.g. `PAIR_DECISION_TIMEOUT`) live here, never in
  one side — a past bug came from each side owning its own deadline.
- `device-host` — the host runtime: TCP listeners and session ownership (`serve.rs`,
  `inject.rs`), Windows screen engine (`desktop/`: Desktop Duplication → D3D11 compute shader
  `composite.hlsl` → NV12 → Media Foundation H.264, fallback GPU → HW+CPU convert → software),
  OS input backends behind `input::InputBackend` (Windows SendInput, macOS Quartz, Linux XTEST),
  and the `keyboard-it-host` console binary.
- `win-receiver` — thin Slint tray/settings shell (`ui/keyboard-it.slint`, WiX in `wix/`) over
  `device-host`. Almost no logic of its own.
- `mac-sender` — the controller: CGEventTap capture + Fn toggle (`capture.rs`), menu bar,
  permissions walkthrough, devices window (`settings.rs`), mDNS discovery, and the remote
  desktop viewer (`desktop/`: network worker + VideoToolbox decode → latest-frame slot →
  AVSampleBufferDisplayLayer on the main thread). Most modules are `cfg(target_os = "macos")`.
- `controller-client` — pinned mobile pairing/session transport, ordered bounded H.264 and input.
- `android-native` — bounded JNI handles for that transport. `android/` is the Android Java UI,
  NSD discovery, MediaCodec renderer and Keystore AES-GCM registry. No background input/clipboard.
- `mac-sender/src/host.rs` — explicit input-only Mac receiver, local pairing consent and offline
  access revocation. Uses separate mobile-host config. Disabled each launch; pauses outbound control.
- `android-adapter` — ADB discovery/pairing and supervised scrcpy processes. Android records
  use ADB's authority and must never carry or activate native Noise credentials.

Three network channels, each with its own Noise handshake:

| Port | Channel | Noise pattern | Wire |
| --- | --- | --- | --- |
| 5599 | legacy input | `NNpsk0` (PSK = BLAKE2s of pairing key) | `protocol::KeyEvent` / tagged `InputEvent`, `secure::PROTOCOL_VERSION` |
| session port + 1 = 5600 (or ephemeral) | pairing | `NN`, unauthenticated; full 256-bit fingerprint from handshake hash + local consent | `protocol::pairing` |
| 5601 (or ephemeral) | remote desktop | `NNpsk0`, domain-separated PSK, versioned Hello | `protocol::desktop` (`VERSION = 2`), 60 KiB chunks, bounded reassembly |

Receivers advertise `_keyboard-it._tcp.local.` with TXT keys for pair/desktop ports, versions and
`device_id`. Discovery is only an **address hint**: identities are pinned after an authenticated
Hello, never from mDNS names.

Invariants that span several files — preserve them when changing behavior:

- **Backward compatibility.** Legacy input v1 and stored credentials remain usable with older 0.5.0
  receivers. New pairing deliberately requires v2 (both apps 0.6.2), with no downgrade
  to the old six-digit fingerprint. New features ride on optional encrypted handshake feature bits
  (`secure::FEATURE_CAPABILITIES`) and optional pairing tails; bump a version constant only for
  truly incompatible changes. Old configs migrate in place (flat sender fields → one-entry
  `devices` list; shared key → separately revocable `legacy-shared-key` group).
- **Bounded everything.** Every allocation/reassembly from network data is bounded before
  accepting it; queues are bounded and fail closed (input preempts clipboard fragments).
- **Stale-work rejection.** Generation/revision counters guard sessions, viewer publication,
  clipboard, Android launch tickets and device selection (A → B → A must not resurrect A's work).
  "Newest connection wins", but only *after* authentication; an unauthenticated socket never
  replaces a live session.
- **Access policy** lives in a `<config>.access.toml` sidecar (separate from UI settings so a
  stale settings save can't restore a revoked controller), is written atomically before taking
  effect, fails closed if corrupt, and is held under a lifetime file lock by the running host.
  Max 32 individual controllers. Revocation closes that controller's legacy + desktop sockets.
- **Capabilities vs permissions.** `capabilities.rs` = what an endpoint implements (authenticated
  exchange); `devices::Permissions` = what the user granted per target. The controller uses the
  intersection. Never advertise a capability that isn't implemented (macOS/Linux hosts are
  input-only).
- **Held input is always released** on disconnect, timeout, revocation or target change; release
  and injection share one lock in `device-host/src/inject.rs`.
- **macOS threading.** AppKit objects are main-thread only. Background threads (network, mDNS,
  pairing, decoding) write `Mutex`/atomic state; main-thread `NSTimer`s poll it into the UI.
  Network workers never activate windows or Spaces, so reconnects can't steal focus.
- **Config transactions.** Sender registry changes go through reload → update → write; the flat
  connection fields are a projection of the active device. Path comes from `Config::path()`
  (`ProjectDirs` `com.keyboard-it.keyboard-it`) unless `KEYBOARD_IT_CONFIG` is set; the console
  host defaults to `host.toml` beside it.
- **Subprocesses** (adb/scrcpy) use argument arrays, never a shell; pairing codes go via stdin
  and are redacted from errors; output and durations are bounded.

## Conventions

- Code, comments and docs are English; user-facing UI strings are Turkish (`Cihazlarım…`,
  `Ekranı Aç`, `Erişimi kaldır`) — keep them Turkish. `docs/android.md` is Turkish.
- Comments explain *why*, often citing the concrete bug that motivated the code; match that.
- `docs/*.md` track what is verified automatically vs on physical devices. Keep those
  statements honest when a change affects them; never mark something physically validated.
- Commit subjects are imperative, sentence-case, outcome-focused (see `git log`).

## Android builds

`packaging/android/build.sh debug` builds ARM64/x86_64 native JNI libraries and runs Android
lint. Requires JDK 21, SDK 35/build-tools 35.0.0, NDK 27.2.12479018 and Android Rust targets.
`packaging/android/sign-release.py` keeps the stable signing key in ignored `.android-signing/`;
never print, commit, upload, regenerate or delete that key/password for routine updates.
CI test-signed APKs are not public release APKs. `publish-apk.py vVERSION` attaches the local
signed APK and updates checksums only after all native/Android CI jobs pass. Gradle version
comes from Cargo.toml. Native libraries must retain 16 KB ELF/ZIP alignment.
The loopback-only `controller-client --example mobile_fixture` never injects OS input and
exists solely for isolated emulator/JNI/MediaCodec tests. See docs/android-controller.md.
