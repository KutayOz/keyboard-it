# Universal device support

## Milestone 1: multiple saved PCs

The Mac menu now opens **Cihazlarım…**. Pair each Windows PC once using the discovery
list and its confirmation code. Pairing another PC preserves all saved pairings and
does not switch away from the PC currently selected. The first pairing selects itself.

Select a saved PC and click **Bu Cihaza Geç**, then use **Ekranı Aç** in the menu.
Saved PCs remain selectable when discovery cannot see them; stored addresses and
authenticated rediscovery still handle reconnection. “Ağda” means an mDNS announcement
was observed, not that the device authenticated or is ready for input. Only one target
is connected/controlled at a time in this milestone.

Each saved PC has local **Ekran**, **Klavye / fare**, and **Metin panosu** controls. Turning
off input permits view-only use. Double-Fn cannot bypass the input setting. Changing
the active PC's settings closes its session; reopen the viewer explicitly. The global
clipboard toggle is also respected.

**Cihazı Unut** removes only the selected local record. Removing the active record
stops control and leaves the target empty; another PC is never selected implicitly.
These are local controls. Receiver-side revocation is separate: use **Erişimi kaldır**
on the receiving Windows PC or `revoke ID` in the console host.

### Migration and session boundaries

- Old sender configs migrate to a one-entry `devices` list with a stable local ID.
  The pairing key, pinned remote identity, host, fallback IP and ports are retained.
- Flat connection fields remain the active-target projection for the existing input
  and desktop transports. Background updates sync only that active record.
- Adding, selecting, forgetting and changing permissions use reload/update/write
  transactions. Concurrent pairing and address updates cannot overwrite the registry.
  Invalid registries/transactions are rejected before replacing the config file.
- Selection revisions reject delayed authentication across A → B → A transitions.
  Runtime generations also reject stale video/clipboard publication. Target changes
  close legacy and desktop sockets, discard their queues, clear the rendered image and
  clipboard baseline, release local capture, and disarm viewer input.
- Discovery labels/IPs cannot overwrite an existing pairing. Deduplication needs a
  pinned identity or the same approved secret at the same host. A newly discovered
  identity is pinned only after an authenticated desktop Hello.
- Legacy input and desktop versions remain unchanged. Optional encrypted handshake
  features gate capability records; optional pairing tails carry approved identities.
  An existing Windows 0.5.0 receiver remains compatible.

### Validation without physical devices

```sh
cargo test --workspace --locked
cargo check --workspace --locked
cargo check -p win-receiver --target x86_64-pc-windows-gnu --tests --locked
# macOS main thread; hidden windows, no input capture/injection or device discovery
cargo run -p mac-sender --locked -- --check-devices
cargo run -p mac-sender --locked -- --check-viewer
```

Tests cover migration persistence, concurrent saves, failed transactions, per-target
settings, stale generations, old socket shutdown, clipboard isolation, and actual
encrypted sessions with two loopback PCs using distinct keys and A → B → A selection.
The devices UI check uses a temporary config and exercises real native controls for
selection, an inactive device's permission change and removal. CI runs these checks.

Physical Mac/Windows focus behavior, real held-key release, LAN/mDNS and firewall
behavior, video performance and OS permission prompts still need device validation.
Automated checks do not establish those behaviors on real PCs.

## Milestone 2: credentials, capabilities and reusable hosts

Every approved controller now gets a separate random key. The receiving PC's settings
list controllers with individual **Erişimi kaldır** controls and **Tüm erişimleri kaldır**.
Re-pairing a stable, locally approved identity rotates only that controller's key.
Matching display names do not merge identities. Existing shared-key installations
migrate to a separately revocable legacy group; re-pair controllers for individual keys.

The access policy is a `*.access.toml` sidecar, separate from UI settings. Atomic
writes complete before approval/revocation succeeds; write failures preserve current
access and do not distribute an unpersisted key. Corrupt policies fail closed. Unix
files are created with mode 0600. A lifetime file lock prevents two host processes or
offline commands from overwriting a running host's cached policy. Use the running
host's UI/console for live revocation.

Revocation closes the affected controller's legacy and desktop sockets. A stale key
snapshot cannot take session ownership afterward; input/remote clipboard dispatch is
serialized against revocation. Pending pairing approvals opened before an access
change are rejected and require a fresh local approval. Other controllers retain access.
The receiver holds at most 32 individual credentials plus the optional legacy group.

Updated desktop peers negotiate authenticated platform, screen, keyboard/mouse,
clipboard and H.264 capabilities. The controller intersects these with each target's
local grants; unsupported actions are disabled. Old peers retain the original desktop
exchange without extra records. Discovery remains an address hint and cannot claim
trusted capabilities. Paired identities come from the approved encrypted pairing;
older receivers can still be pinned by their authenticated desktop Hello.

`device-host` now owns the transport, pairing, session ownership, Windows screen engine
and native input adapters. The Windows tray uses this shared library. Its OS-independent
`InputBackend` interface also has macOS Quartz and Linux X11/XTEST implementations.
These two adapters currently receive keyboard/mouse events only. They do not advertise
screen capture or clipboard support. Wayland is rejected before opening Xwayland.

| Endpoint | Controller | Host input | Host screen / clipboard |
| --- | --- | --- | --- |
| macOS menu app | Implemented | Console host | Pending |
| Windows tray | Pending | Implemented | Implemented |
| Linux X11 console | Pending | Implemented | Pending |
| Linux Wayland | Pending | Pending | Pending |
| Android via ADB / scrcpy | Pending | Managed scrcpy input | Managed scrcpy screen; auto clipboard off |
| iOS / TV / smart home | Pending | Pending | Pending |

Native injection on the added macOS/Linux adapters has **not** been tested on physical
devices. The platform enum also includes future families; this does not advertise
implemented iOS, television or smart-home adapters. Android now has the explicit ADB/scrcpy adapter described below.

### Console host

```sh
cargo run -p device-host --bin keyboard-it-host -- --help
cargo run -p device-host --bin keyboard-it-host
cargo run -p device-host --bin keyboard-it-host -- --dry-run --config /tmp/kbit-host.toml
```

Default console config is `host.toml` next to the app config, independent of the
controller's config. `--config PATH` or `KEYBOARD_IT_CONFIG` overrides it. Native
macOS startup checks Accessibility permission without opening an OS prompt. Linux
requires a signed-in X11 desktop with libX11, libXtst and XTEST; no root access or
shell-based injection is used. Use the Mac's **Yalnız klavye/fare modu** with these hosts.

Every console pairing, including dry runs, requires local `allow CODE` after comparing
the two screens. Wrong, expired and replayed approvals fail. `deny` refuses the pending
request. `list`, `revoke ID` and `revoke-all` manage live access; `quit` or EOF stops the
host and releases held input. Offline access flags refuse to run while the same policy
is owned by another process. Console hosts are source builds; native installers remain
for the current Mac controller and Windows tray.

### Added device-free validation

```sh
cargo check -p device-host --target x86_64-unknown-linux-gnu --all-targets --locked
cargo check -p win-receiver -p device-host --target x86_64-pc-windows-gnu --tests --locked
# CI's dedicated software X server, never a user's desktop:
env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 KEYBOARD_IT_VIRTUAL_DISPLAY=1 \
  xvfb-run -a cargo test -p device-host --test x11_input --locked -- --ignored
```

Loopback tests exercise individual keys, revocation/re-pairing, persistence failures,
stale handshakes/approvals, legacy compatibility and authenticated capability exchange.
The console subprocess fixture runs with `--dry-run` and checks startup, clean shutdown
and refusal of concurrent policy edits. Recording input adapters check held-input
release and stale-owner isolation. CI additionally runs the X11 adapter against isolated
Xvfb and reads key/button state back from the virtual server. Local cross-compilation
checks its code; Xvfb execution requires Linux and is reported separately from local tests.

## Milestone 3: Android through ADB / scrcpy

**Android Ekle…** in the Mac's **Cihazlarım…** window discovers explicitly authorized
ADB devices after the user requests a refresh, saves a selected ready device, and
supports wireless debugging pairing/connection. The selected Android is opened through
**Ekranı Aç** in a separately managed scrcpy window. Details and dependencies:
[Android setup](android.md).

The registry has a typed `native` / `adb` transport. Android records use ADB's authority,
carry no native shared secret, and never activate native input or desktop connections.
Adding a phone preserves an active PC. Reapproving the same Android identity at a new
USB/Wi-Fi serial preserves the local ID and grants, and invalidates the old session.
Saved identities are rechecked before launch; there is no fallback to a different phone.

Screen/input grants map to refusing launch or enabling read-only mode. Audio and automatic
clipboard synchronization remain off. User-initiated scrcpy paste shortcuts are a separate
operation, documented in the Android guide. Target changes, forgetting, permission edits,
pause and quit cancel the owned viewer. Generation checks reject stale probes and stale
watcher cleanup. Commands use argument arrays; pairing codes use stdin and are redacted
from errors. Tool output and command durations are bounded.

These paths are tested with fake ADB output and controlled software processes. The hidden
AppKit devices check now covers Android setup and selection without running ADB/scrcpy.
On 2026-10-26, an SM-G780G running Android 13 was added and selected through the real
AppKit flow over USB. The user confirmed live video and mouse control in the managed
window. The device manager now also provides an **Ekranı Aç** button for the active
device with screen permission. Wi-Fi pairing, keyboard behavior and other OEMs remain
physically unvalidated.
Android uses an external tool window; direct native Android video and an Android controller
app are still future work.

## Remaining steps

1. macOS/Linux screen and clipboard backends, Wayland portal support, and Windows/Linux
   controller applications. Keep capability reports limited to implemented operations.
2. Home Assistant adapters and explicitly supported TV backends; an Android controller
   and direct Android video backend. iOS controller/screen-sharing and full input control
   need separate capability work.
3. An authenticated controller API and mobile/web clients, followed by explicit
   multi-device actions. Screen streams remain peer-to-peer.
