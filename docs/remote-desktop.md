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

## Picture quality and performance (0.5.0)

Both applications must be updated to 0.5.0 for desktop protocol v2. Saved pairing keys
are preserved. Older receivers still support **Yalnız klavye/fare modu**.

The Mac menu **Görüntü kalitesi** offers:

| Profile | Maximum dimensions | Target | Bitrate |
| --- | --- | --- | --- |
| Doğal çözünürlük · 60 FPS (default) | Source, capped proportionally at 4096 | 60 FPS | Automatic, based on actual dimensions/FPS |
| Doğal çözünürlük · 120 FPS | Same | 120 FPS | Automatic |
| 1080p60 · 20 Mbps | 1920×1080, no upscaling | 60 FPS | 20 Mbps |
| 1440p60 · 40 Mbps | 2560×1440, no upscaling | 60 FPS | 40 Mbps |

Changing a profile reconnects without replacing or focusing the viewer. Hardware, active
Windows monitor refresh rate, Mac display refresh rate and network throughput limit usable
FPS. In particular, sending 120 FPS to a 60 Hz display cannot show 120 distinct frames.
Unsupported encoder FPS fall back to 60/30 before reducing resolution. The connection
status reports effective dimensions, effective encoder FPS, target bitrate and processing path.

**Performans bilgilerini göster** displays measured decode/presentation-submission rates,
bitrate, decode CPU duration and application ping/pong RTT. **Performans ölçümlerini kaydet…**
saves up to 300 one-second samples as CSV using a native save dialog. Samples stay in memory;
a new connection/profile resets them. No keys, clipboard, images, addresses or pairing secrets
are logged. “submitted_fps” counts display-layer submissions, not physical monitor scanout.
Host capture/encode/send durations measure CPU wall time, not GPU completion. RTT includes
application scheduling and video TCP backpressure; it is not input-to-photon latency.

For a repeatable test, start with 1080p60/20 Mbps, wait 10 seconds after connecting, then
scroll text, drag a window and play a moving test pattern for 60 seconds. Export the CSV,
repeat at 1440p60/40 Mbps and native 60/120, and record Windows GPU/driver, both displays'
refresh rates, network type, effective encoder path and resolution. A high-speed external
camera measuring a physical input and resulting display change is required for true
input-to-visible-response latency. Results on the user's physical PC are not yet measured.

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
- Desktop Duplication keeps an owned D3D11 copy before releasing the captured frame.
  A compute shader rotates, scales and composites color/masked/monochrome cursor shapes.
  The D3D11 video processor converts to limited-range BT.709 NV12 on the GPU. A shared
  DXGI device manager supplies those surfaces directly to a D3D11-aware Media Foundation
  hardware encoder, without reading desktop pixels back to system memory.
- Encoder-owned samples use a bounded 2–4-surface allocator. Captures are skipped when
  surfaces/input credits are unavailable; in-flight textures are not overwritten. Async MFT
  events are polled without the former 100 ms waits. Monotonic timestamps describe actual
  submission time. Capture pacing skips missed deadlines and recovers target FPS after load.
- The fallback order is GPU surfaces, hardware encoding with CPU conversion, then software.
  A runtime GPU failure retries with CPU conversion; hardware encoder failure retries software.
  The compatibility path preserves cursor/rotation handling and uses row copies for unrotated
  displays. Software conversion uses BT.601 with matching metadata. H.264 remains the codec;
  HEVC/AV1, 4:4:4 and a datagram video transport are not implemented in this version.
- H.264 access units are chunked into at most 60 KiB payloads and assembled with a 16 MiB
  frame bound. The receiver requires contiguous chunks and a five-second transfer deadline.
  Encoded reference frames are decoded in order; only already decoded images are replaced
  in the latest-frame slot. There is no unbounded video queue.
- VideoToolbox produces retained immutable pixel buffers off the main thread. AppKit presents
  samples immediately through AVSampleBufferDisplayLayer. On macOS 14+, NSView's display link
  follows the current display with a preferred rate up to 120 Hz. Older systems use a 120 Hz
  timer in common run-loop modes, including during dragging/menu tracking. All windows,
  focus, Spaces and cursor visibility changes stay on the main thread.
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
Media Foundation encoder without requiring screen capture. WARP compute tests compile and
execute the production cursor/rotation shader for four orientations and color, masked and
monochrome cursor operations without requiring a physical GPU. Settings bounds, deadline
recovery, telemetry wire records and bounded CSV reporting also have automated tests. Native macOS and Windows CI jobs
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
becoming reachable; at 1080p/60 FPS, input-to-visible-response at or below 150 ms. These are
measurement targets, not verified guarantees. Record hardware, effective resolution, encoder,
network conditions and observed latency during physical-device validation. Headless/virtual
monitors, WAN connectivity, audio, files and monitor selection are outside this version.
