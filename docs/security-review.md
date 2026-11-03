# LAN security review — 2026-11-04

## Scope and conclusion

Reviewed the pending universal-device changes and the native Mac → Windows network paths,
including pairing, input, desktop/clipboard, discovery, access revocation and installer firewall
configuration. This is a source review with local adversarial regression tests, not an independent
cryptographic audit, a penetration test of the user's router, or physical Windows validation.

Native traffic was already encrypted. Adding a second custom encryption layer would not address
the concrete issues below. Version 0.6.2 strengthens authentication and source-address scope while
retaining Noise and existing session credentials. No evidence was found in the inspected native
paths that a LAN observer without a pairing credential can decrypt a completed session or inject
valid transport records. That bounded conclusion is not a claim that all software is vulnerability-free.

## Existing protections and why they matter

- Input uses `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s` through `snow`; the screen/clipboard channel
  derives a separate PSK. Generated pairing credentials contain 32 bytes from the OS RNG.
  The PSK authenticates the peer; ephemeral DH derives fresh session keys; AEAD rejects modified
  ciphertext. Fresh session keys and ordered transport nonces prevent recorded input from being
  accepted again. This is independent of whether Wi-Fi itself uses WPA encryption.
- The desktop channel requires an encrypted Hello before publishing session ownership. Discovery
  names, IP addresses and mDNS device IDs are only routing hints; saved identity checks happen
  after the authenticated channel exchange. Spoofing a service name alone does not grant control.
- Individual credentials are persisted before successful pairing, revocation is persisted before
  applying it, and attachment rechecks current policy. Revocation shuts down that controller's
  sockets and blocks further dispatch, including stale handshakes. The legacy shared-key group
  remains separately revocable for migrations.
- Network records, video/clipboard reassembly and concurrent handshake counts are bounded.
  These limit resource use; they do not make the service immune to denial of service.

## Findings, causal chains and changes

### 1. Captured input handshake could evict the current controller — reproduced and fixed

**Precondition:** an attacker can capture a valid first input-handshake message and reach the
input listener. No pairing key or ephemeral private key is required for the replay itself.

**Cause:** the first `NNpsk0` message is authenticated with a PSK but has no fresh contribution
from the receiver. It can be replayed. `handle_client` previously acquired input ownership and
closed the current socket immediately after sending its handshake response, before receiving
proof that the client knew the new session's transport key.

**Effect:** a captured first flight could disconnect the legitimate controller and release its
held input. The attacker still could not decrypt the screen or send valid input. This is an
availability/ownership issue, not an encryption break.

**Evidence:** `serve::access_tests::replayed_handshake_cannot_displace_a_live_input_session`
starts a legitimate session, replays a recorded first flight on another socket and checks that
the original session retains ownership. It failed on the pre-fix host with
`replaying a captured handshake displaced the live controller`; it passes after the fix.
The test uses loopback sockets and the dry-run input adapter, never real OS input.

**Fix:** require a valid encrypted transport event before attaching the credential or replacing
any session. Updated Mac controllers send a zero-motion event immediately. The original first
event is dispatched once after authorization. Pre-authentication reads and writes have five-second
socket timeouts. Older controllers may reconnect if they idle longer than that before their first
event; existing wire formats and keys remain unchanged. The desktop channel already required a
fresh encrypted Hello and did not have this particular ownership bug.

### 2. Six-digit uncommitted pairing fingerprint was too small — design weakness addressed

**Precondition:** an active intermediary is present during initial pairing and can terminate two
separate unauthenticated NN handshakes. Subsequent correctly paired sessions are PSK-authenticated.

**Cause:** the previous comparison code reduced a handshake digest modulo 1,000,000, leaving
about 20 bits. A different transcript can have the same displayed code. An intermediary can vary
its ephemeral contribution offline to search candidate transcripts before presenting a matching
code; dialog cooldown does not rate-limit that offline work. The exact attack cost depends on
scheduling and implementation. No full key-grinding MITM exploit or hardware benchmark was run
in this review; the small output space follows directly from the code. Previous comments saying
two MITM handshakes *cannot* match were incorrect.

**Effect:** comparing six digits was not as strong as the documentation claimed. If a matching
pair of transcripts is found and pairing approved, the intermediary can learn the new PSK.
Simply replacing NN encryption with another unauthenticated encrypted channel would not solve it.

**Fix:** pairing v2 displays a domain-separated full 256-bit transcript fingerprint: eight groups
of eight hexadecimal characters. Users must compare **every group on both physical screens**.
Generic collision search against a full 256-bit digest requires roughly 2^128 work under the
usual hash assumptions; this is a security estimate, not an empirical benchmark or proof of the
entire application. Windows wraps the complete fingerprint; a native Mac check verifies that
the label retains the complete string and has sufficient width. New pairing refuses v1 rather
than silently downgrading. Both apps must be 0.6.2 for new pairings; saved session keys still work.

**Limit:** approving without comparison, or comparing only a prefix, still allows MITM. The longer
fingerprint trades convenience for a large verification space without inventing a PAKE/commitment
protocol. Existing credentials are not retroactively proven safe: if an earlier pairing occurred
on a hostile network, revoke its access and pair again after updating both devices.

### 3. Windows firewall allowed any source address — configuration corrected

**Cause:** the MSI created a program exception with `Scope='any'` and `Profile='all'`, while
listeners bind `0.0.0.0`. Same-network discovery did not enforce a LAN-only reachability boundary.

**Effect:** if routing, a VPN, or router port forwarding made the listener reachable, the installer
rule did not limit clients to the local subnet. This did not bypass Noise authentication, but
unnecessarily exposed handshake and pairing endpoints beyond the advertised LAN scope.

**Fix:** the installer uses `Scope='localSubnet'`; all profiles remain allowed because Windows
can classify a trusted home Wi-Fi as Public. This is a source-address restriction, not peer
identity authentication. Different routed subnets may now require an explicitly configured rule.
Native Windows CI validates packaging; the effective firewall rules on the user's actual PC,
including old/manual rules and third-party firewalls, still require physical inspection.

### 4. Diagnostic receiver silently approved pairings — removed

The non-Windows `win-receiver` diagnostic path printed the code and automatically accepted
requests. It did not inject real input, but persisted credentials without user verification.
It now declines new pairings. The console host's explicit `allow CODE` flow is the supported
device-free pairing path. Production Windows and the console host already required consent.

## Verification

Commands for this release:

```sh
cargo test --workspace --locked
cargo run -p mac-sender --locked -- --check-devices
cargo run -p mac-sender --locked -- --check-viewer
cargo check -p win-receiver -p device-host --target x86_64-pc-windows-gnu --tests --locked
cargo check -p android-adapter -p protocol -p mac-sender -p device-host --target x86_64-unknown-linux-gnu --all-targets --locked
```

The added transport tests reject a wrong PSK, modified ciphertext, same-session replay and
cross-session replay. Existing tests exercise individual revocation, stale attachment rejection,
identity binding, bounded parsing, clipboard handling and desktop session isolation. CI additionally
runs native Windows encoder/GPU tests and a Linux Xvfb input test before release publication.
Passing tests establish their tested properties, not universal security or measured LAN performance.

## Dependency advisory triage

The locked registry packages were queried against OSV/RustSec on 2026-11-04 (633 package
versions before updates). Matching an advisory is evidence about a dependency version, not
proof that the application's LAN protocol exposes its affected API.

Compatible patched versions were applied in the lockfile:

| Package | Before → after | Advisory and scope |
| --- | --- | --- |
| event-listener | 5.4.1 → 5.4.2 | [RUSTSEC-2026-0221](https://rustsec.org/advisories/RUSTSEC-2026-0221.html), unsound thread transfer of non-Send tags |
| pulldown-cmark | 0.13.4 → 0.13.6 | [RUSTSEC-2026-0414](https://rustsec.org/advisories/RUSTSEC-2026-0414.html), [RUSTSEC-2026-0433](https://rustsec.org/advisories/RUSTSEC-2026-0433.html), Markdown wikilink DoS; reached via Slint's build compiler dependency |
| webbrowser | 1.2.1 → 1.2.2 | [RUSTSEC-2026-0257](https://rustsec.org/advisories/RUSTSEC-2026-0257.html), Unix browser argument injection; no application-controlled URL opening was identified in the native LAN paths |

Remaining lockfile matches are recorded rather than silently ignored:

- `quick-xml 0.39.4`: [RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194.html)
  and [RUSTSEC-2026-0195](https://rustsec.org/advisories/RUSTSEC-2026-0195.html), XML CPU/memory
  DoS, patched in 0.41.0. `cargo tree -i quick-xml --target all --locked` traces it exclusively
  through the Wayland proc-macro `wayland-scanner`, behind Slint's Linux GUI backend. This
  workspace enables Slint only on Windows; the actual Windows and Linux target dependency
  trees have no active quick-xml consumer. It is not the native LAN record parser. No remote
  exploitability in the shipped Mac/Windows apps was established. The inactive lockfile entry
  should be revisited with future upstream framework upgrades.
- Seven **unmaintained** notices cover `bincode`, `paste`, `ttf-parser`, `rustybuzz`, `zvariant`,
  `zbus_names` and `zvariant_derive` (RUSTSEC-2025-0141, 2024-0436, 2026-0192, 2026-0206,
  2026-0370, 2026-0371, 2026-0372). These are upstream maintenance/migration work, not seven
  demonstrated attacks. Their advisories provide no patched version; replacing the GUI's
  transitive font and D-Bus stack needs a separate tested framework migration.

The initial 0.6.0 tag's build was cancelled before release publication to include this triage
and the compatible security updates in the final release. The 0.6.1 run then caught a
Windows-incompatible failure-injection test setup (renaming the parent of an open lock file).
The fixture now obstructs the policy replacement path directly on all platforms. It still
requires failed writes to preserve live access; no assertion was removed. Final publication
uses 0.6.2. No released tag was rewritten.

## Download/install hardening

The terminal installer now pins both its DMG and checksum manifest to the same release,
uses a private `mktemp -d` directory rather than predictable `/tmp` paths, and verifies the
SHA-256 digest before mounting or replacing an existing app. A mocked-tool installer check
covered success, mismatched digest, manifest download failure, a missing entry and duplicate
entries; every failure left the previous fixture app intact and never mounted/copied/launched.
No test installed over the user's real app. Checksums remain same-origin integrity checks,
not a replacement for publisher signing.

## Remaining boundaries

- A malicious router or LAN peer can drop/delay packets, jam discovery, occupy bounded handshake
  slots or repeatedly request pairing. Socket timeouts are per operation, not absolute connection
  deadlines. Encryption does not guarantee availability; further admission controls would be
  needed for untrusted networks. No claim of DoS resistance is made.
- mDNS names/addresses, packet lengths and timing remain visible. Wi-Fi membership alone is not
  identity, and source-subnet scope does not make other LAN clients trustworthy.
- Credentials are plaintext in local config/access files (0600 on Unix; Windows inherits directory
  ACLs). Keychain/DPAPI storage is not implemented. Malware running as the authorized OS user can
  steal keys or input. Legacy manually chosen weak secrets permit offline guessing; use the
  generated 256-bit pairing keys, not a short `KEYBOARD_IT_KEY` password.
- Installers remain unsigned/not notarized. HTTPS GitHub delivery and published SHA-256 checksums
  aid artifact verification but do not replace signed publisher identity or protect a compromised
  GitHub account. Checksums from the same compromised origin cannot solve that problem.
- Android uses external ADB/scrcpy, not the native Noise channel. The app cannot claim Noise
  encryption for a pre-existing legacy ADB TCP connection. Use Android 11+ Wireless debugging
  pairing (TLS) for Wi-Fi, or USB; avoid legacy `adb tcpip 5555` on an untrusted LAN. Android
  Wi-Fi behavior and external tool versions were not physically verified in this review.
- Native physical Mac/Windows pairing, Windows firewall migration and the updated Windows dialog
  layout must still be exercised on actual devices. Earlier user testing predates these changes.

## Primary references

- [Noise specification: channel binding](https://noiseprotocol.org/noise.html#channel-binding),
  [application responsibilities](https://noiseprotocol.org/noise.html#application-responsibilities)
  and [security considerations](https://noiseprotocol.org/noise.html#security-considerations).
- [WiX FirewallException scope](https://docs.firegiant.com/wix3/xsd/firewall/firewallexception/).
- [Android ADB Wi-Fi architecture](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/docs/dev/adb_wifi.md).
