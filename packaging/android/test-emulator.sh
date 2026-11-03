#!/usr/bin/env bash
# Run only on isolated emulators; never install test APKs onto a physical phone.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
: "${ANDROID_SERIAL:?Set ANDROID_SERIAL to the isolated emulator}"
case "$ANDROID_SERIAL" in emulator-*) ;; *) echo 'Tests require an emulator, not a physical device' >&2;exit 1;; esac
[[ "$(adb -s "$ANDROID_SERIAL" shell getprop ro.kernel.qemu | tr -d '\r')" == 1 ]]
if adb devices | awk 'NR>1 && $2=="device" && $1 !~ /^emulator-/ {found=1} END {exit !found}'; then
    echo 'Disconnect physical Android devices before running connected tests' >&2;exit 1
fi
python3 - <<'PY'
import socket
for port in (57811,57812,57821,57822):
    with socket.socket() as test:
        test.bind(('127.0.0.1',port))
PY
cargo build -p controller-client --example mobile_fixture --release --locked
"$ROOT/target/release/examples/mobile_fixture" > "${TMPDIR:-/tmp}/keyboard-it-fixture-test.log" 2>&1 &
FIXTURE_PID=$!
trap 'kill "$FIXTURE_PID" 2>/dev/null || true' EXIT
python3 - <<'PY'
import socket,time
end=time.monotonic()+15
while True:
    try:
        with socket.create_connection(('127.0.0.1',57811),timeout=.2):break
    except OSError:
        if time.monotonic()>end:raise
        time.sleep(.1)
PY
cd android
./gradlew connectedDebugAndroidTest --no-daemon
