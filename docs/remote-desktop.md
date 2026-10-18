# Remote desktop

## Install and use

1. Install the updated `.dmg` on the Mac and `.msi` on Windows. Keep both computers on the
   same LAN. Windows must be awake, signed in, with its primary display active.
2. Finish the Mac permissions guide. In Settings choose the Windows PC and Pair & Connect.
   Compare the short code displayed on both computers, then approve on Windows.
3. Start at login is selected by default during first setup. Windows asks for the scheduled
   task's permission once; declining it leaves the current pairing usable. Retry from
   Windows Settings if needed. The task runs in the interactive user session.
4. Choose **Windows’u Aç** in the Mac menu bar. This first action opens a native full-screen
   Space visible in Mission Control. The green window button switches between full screen
   and a normal resizable window.
5. While the viewer is focused and a current frame is visible, input goes to Windows.
   Change Spaces, switch applications, close the viewer, or double-tap Fn to release input.
   Ctrl+arrow Space shortcuts remain local. Double-tap Fn again to resume input.
6. Copy plain text normally. **Metin panosunu paylaş** turns automatic text sharing on/off.
   Sharing pauses when the viewer loses focus. Images/files and text above 1 MiB are ignored.

The connection is maintained in the background even before opening the viewer. Reconnection
keeps an existing viewer/Space in place and never activates it. **Bağlantıyı duraklat** stops
connection attempts until unchecked. **Yalnız klavye/fare modu** restores the original
relative-pointer, double-Fn workflow, including with an older Windows receiver.

The Windows physical screen remains the same interactive desktop: this does not create a
separate Windows account/session or blank the physical monitor. Login, UAC's secure desktop,
and a missing active monitor cannot be controlled. The viewer shows a status and retries
capture when the normal desktop is restored. No additional Mac Screen Recording permission
is required because the captured screen is on Windows.

## Implementation

- Existing pairing and input protocol v1 remain unchanged. The desktop protocol has its own
  versioned Hello exchange on a separate Noise channel; its PSK is domain-separated from the
  legacy channel. Existing config files receive defaults, not a new pairing key.
- Windows advertises `device_id`, `desktop_version`, and `desktop_port` in the existing mDNS
  service. The desktop listener prefers 5601 and falls back to a dynamically assigned port.
  The installer's program firewall rule also covers that port.
- mDNS and stored hostname/IP are address hints. A saved identity is matched during rediscovery;
  the PSK and authenticated Hello must verify before a new address/identity is persisted.
  On migration, the first authenticated desktop session learns the identity.
- Desktop Duplication captures the primary output in its logical orientation and composites
  color/masked/monochrome cursor shapes. BGRA is converted to NV12 for Media Foundation.
  The encoder tries hardware, then software. Runtime hardware encoder failures fall back to
  software. Unsupported sizes are reduced proportionally; effective resolution is shown in
  the window title. The target is source resolution at 30 FPS (maximum encoder dimensions
  4096); load reduces frame rate before resolution.
- H.264 access units are chunked into at most 60 KiB payloads and assembled with a 16 MiB
  frame bound. The receiver requires contiguous chunks and a five-second transfer deadline.
  Encoded reference frames are decoded in order; only already decoded images are replaced
  in the latest-frame slot. There is no unbounded video queue.
- VideoToolbox produces retained immutable pixel buffers off the main thread. AppKit presents
  samples immediately through AVSampleBufferDisplayLayer. All windows, focus, Spaces and
  cursor visibility changes stay on the main thread.
- Each input event checks focus; a timer is not the only gate. Pointer coordinates account
  for letterboxing and the Windows primary display's position inside the virtual desktop.
  Mouse-up outside the image still releases its Windows button. Menu bar/Dock overlays retain
  local pointer access. Legacy and desktop connections share one generation-based input owner.
- Heartbeats run every second; reads/writes time out after five seconds. The Mac releases input
  and reconnects if no decoded frame arrives for three seconds while viewing. Receiver socket
  closure/timeout also releases held inputs. The bounded input queue fails closed on overflow;
  input/control preempt outgoing clipboard fragments.
- Clipboard synchronization baselines are reset on focus loss, connection and sharing changes.
  New Mac text copied while the Space is inactive is shared only after returning to it;
  reconnecting still discards pending old clipboard content.
  Remote transfer IDs and content tracking suppress echoes. Local clipboard text is never
  replayed merely because a connection was re-established. Text is UTF-8 on the wire,
  Unicode text on Windows and a native string on macOS; limit 1 MiB, no NUL characters.

## Validation

Automated commands:

```sh
cargo test --workspace --locked
cargo check --workspace --locked
# macOS-only: exercise AppKit + VideoToolbox + AVSampleBufferDisplayLayer on the main thread
cargo run -p mac-sender --locked -- --check-viewer
# Optional cross-platform type checking from macOS (install this Rust target first):
cargo check -p win-receiver --target x86_64-pc-windows-gnu --tests --locked
```

The suite covers protocol limits/reassembly, mismatched versions/keys, authenticated
reconnection, unauthenticated socket isolation, port collision fallback, stale-owner cleanup,
clipboard echo/reconnect behavior, pointer mapping, and real VideoToolbox decoding using a
small generated H.264 test-pattern fixture. A main-thread native viewer smoke check also
validates full-screen window configuration and the Objective-C display-layer calls without
showing a window or requesting input permissions. Windows-native tests also exercise the software
Media Foundation encoder without requiring screen capture. Native macOS and Windows CI jobs
run tests before packaging `.dmg` and `.msi` artifacts.

Manual acceptance on a real Mac/Windows pair is still required:

| Scenario | Expected result |
| --- | --- |
| First pairing and restart of both apps/OS sessions | One approval; reconnect without IP/key entry |
| Sleep/wake, Wi-Fi reconnect, DHCP address change | Rediscover and authenticate the same PC |
| Switch Spaces/applications or close/minimize viewer | Mac input restored; no stuck Windows keys/buttons |
| Network cut while dragging or holding modifiers | Input released; viewer retained; fresh frame before resuming |
| Copy Turkish/emoji text in both directions | Correct text, no echo, no sharing while unfocused |
| Disable/re-enable sharing or reconnect | Existing clipboard not replayed |
| Letterboxing, Retina scaling, rotated primary display | Pointer/cursor and clicks align; outside-image release works |
| Hardware encoder unavailable/fails | Software fallback or a clear capture status |
| Lock/UAC and return to desktop | No frozen-screen input; capture automatically recovers |
| Older receiver | Clear update message; input-only mode remains usable |

Performance acceptance targets on a healthy LAN: reconnect within 10 seconds of the PC
becoming reachable; at 1080p/30 FPS, input-to-visible-response at or below 150 ms. These are
measurement targets, not verified guarantees. Record hardware, effective resolution, encoder,
network conditions and observed latency during physical-device validation. Headless/virtual
monitors, WAN connectivity, audio, files and monitor selection are outside this version.
