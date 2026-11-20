#!/usr/bin/env bash
# Builds Chippy's APK: the codec libraries (once, see
# packaging/build-codecs.sh), the Rust engine with cargo-ndk, its Kotlin
# bindings with UniFFI, then the Compose app with Gradle.
#
#   android/build-apk.sh [debug|release]
#
# Needs ANDROID_HOME (SDK with platform 35 and build-tools), ANDROID_NDK_HOME,
# the Rust target aarch64-linux-android, cargo-ndk, and Gradle 8.
# The APK lands in android/app/build/outputs/apk/<type>/.
set -euo pipefail

TYPE="${1:-release}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"
ABI=arm64-v8a
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to the Android NDK}"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"

DEPS="${CHIPPY_CODECS:-$ROOT/build/codecs/$ABI}"
if [ ! -f "$DEPS/lib/libavcodec.a" ]; then
  "$ROOT/packaging/build-codecs.sh" "$ABI" "$DEPS"
fi
export CHIPPY_CODECS="$DEPS"

PROFILE=(--release)
[ "$TYPE" = debug ] && PROFILE=()
OUT="$HERE/build/jni"
GEN="$HERE/app/build/generated/uniffi"
rm -rf "$OUT" "$GEN"
(
  cd "$ROOT"
  # Unstripped: UniFFI reads the bindings' description from the library's
  # symbols. The copy in the APK is stripped below.
  CARGO_PROFILE_RELEASE_STRIP=false cargo ndk -t "$ABI" -P 26 -o "$OUT" \
    build "${PROFILE[@]}" --locked -p chippy-android
  # The Kotlin bindings (io.github.singularity098.chippy.core).
  cargo run --locked -q -p uniffi-bindgen -- generate \
    --library "$OUT/$ABI/libchippy_android.so" \
    --language kotlin --out-dir "$GEN" --no-format
)
ls -l "$GEN"/io/github/singularity098/chippy/core/
# Only Chippy's library, and the C++ runtime it (and Oboe) link to: cargo-ndk
# also copies dylibs some dependencies build for themselves.
LIBS="$HERE/app/src/main/jniLibs/$ABI"
rm -rf "$LIBS"
mkdir -p "$LIBS"
cp "$OUT/$ABI/libchippy_android.so" "$LIBS/"
LLVM="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64"
"$LLVM/bin/llvm-strip" --strip-all "$LIBS/libchippy_android.so"
case "$ABI" in
  arm64-v8a) TRIPLE=aarch64-linux-android ;;
  x86_64) TRIPLE=x86_64-linux-android ;;
esac
cp "$LLVM/sysroot/usr/lib/$TRIPLE/libc++_shared.so" "$LIBS/"
ls -l "$LIBS"

cd "$HERE"
TASK="assemble$(tr '[:lower:]' '[:upper:]' <<< "${TYPE:0:1}")${TYPE:1}"
# Once more if the first try fails: Maven repositories have bad moments.
"${GRADLE:-gradle}" --no-daemon "$TASK" || "${GRADLE:-gradle}" --no-daemon "$TASK"
ls -l "app/build/outputs/apk/$TYPE"
