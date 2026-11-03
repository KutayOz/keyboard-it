#!/usr/bin/env bash
# Build the existing Rust protocol into an installable native Android app.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
: "${ANDROID_HOME:?Set ANDROID_HOME to the SDK directory}"
NDK="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/27.2.12479018}"
case "$(uname -s)" in Darwin) HOST=darwin-x86_64;; Linux) HOST=linux-x86_64;; *) echo 'Build on macOS or Linux' >&2;exit 1;; esac
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/$HOST/bin"
MODE="${1:-debug}"
if [[ "$MODE" != debug && "$MODE" != release ]]; then echo 'Use debug or release' >&2;exit 1;fi
for SPEC in 'aarch64-linux-android:arm64-v8a:aarch64-linux-android26-clang' 'x86_64-linux-android:x86_64:x86_64-linux-android26-clang'; do
    IFS=: read -r TARGET ABI CLANG <<< "$SPEC"
    LINKER_VAR="CARGO_TARGET_$(echo "$TARGET" | tr '[:lower:]-' '[:upper:]_')_LINKER"
    env "$LINKER_VAR=$TOOLCHAIN/$CLANG" RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-z,max-page-size=16384" cargo build -p android-native --target "$TARGET" --release --locked
    mkdir -p "android/app/src/main/jniLibs/$ABI"
    cp "target/$TARGET/release/libkeyboard_it_mobile.so" "android/app/src/main/jniLibs/$ABI/"
done
cd android
if [[ "$MODE" == release ]]; then TASK=assembleRelease; else TASK=assembleDebug; fi
./gradlew "$TASK" lint --no-daemon
