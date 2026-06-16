#!/usr/bin/env bash
# Regenerate the engine artifacts the Gradle build consumes (both git-ignored):
#   1. the UniFFI Kotlin binding  -> app/src/uniffi/kotlin
#   2. the engine .so per ABI     -> app/src/main/jniLibs
# Run from anywhere; paths are resolved relative to the repo root.
#
# Prereqs: rustup android targets + `cargo install cargo-ndk` and an Android NDK
# (ANDROID_NDK_HOME). CI runs these same two steps before invoking Gradle.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cd "$root"

abis="${ABIS:-arm64-v8a}"

echo ">> building host lib for binding metadata"
cargo build -p abbrev-ffi --release

echo ">> generating Kotlin binding"
rm -rf "$here/app/src/uniffi/kotlin"
cargo run -p uniffi-bindgen -- generate \
  --library target/release/libabbrev_ffi.so \
  --language kotlin \
  --no-format \
  --out-dir "$here/app/src/uniffi/kotlin"

echo ">> building engine .so for: $abis"
ndk_args=()
for abi in $abis; do ndk_args+=(-t "$abi"); done
cargo ndk "${ndk_args[@]}" -o "$here/app/src/main/jniLibs" build -p abbrev-ffi --release

echo ">> done. now: (cd $here && ./gradlew assembleDebug)"
