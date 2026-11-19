#!/usr/bin/env bash
# Builds the codec libraries vgmstream uses (FFmpeg, mpg123, libvorbis and
# libogg) as static libraries, the way vgmstream's own CMake builds them for
# its static releases: for Android, and for the Linux release binary (so it
# runs whatever FFmpeg the system has, or none).
#
#   packaging/build-codecs.sh [TARGET] [PREFIX]
#
# TARGET is an Android ABI (arm64-v8a, the default, or x86_64) or `linux`
# (this machine); PREFIX defaults to build/codecs/<TARGET>. Needs cmake,
# make, git and pkg-config; for Android, ANDROID_NDK_HOME (or
# ANDROID_NDK_ROOT). Point CHIPPY_CODECS at PREFIX when building Chippy.
set -euo pipefail

ABI="${1:-arm64-v8a}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"
PREFIX="${2:-$ROOT/build/codecs/$ABI}"
API=26

case "$ABI" in
  arm64-v8a) TRIPLE=aarch64-linux-android; FFARCH=aarch64 ;;
  x86_64) TRIPLE=x86_64-linux-android; FFARCH=x86_64 ;;
  linux) ;;
  *) echo "unsupported target $ABI" >&2; exit 1 ;;
esac
if [ "$ABI" != linux ]; then
  NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
  [ -d "$NDK" ] || { echo "set ANDROID_NDK_HOME to the NDK" >&2; exit 1; }
  TC="$NDK/toolchains/llvm/prebuilt/linux-x86_64"
fi

# Kept in step with third_party/vgmstream/cmake/dependencies/*.cmake.
FFMPEG_TAG=n9.0.2
MPG123_COMMIT=295c21d649b04ef0e242df7eee259b5a9651f69c
OGG_TAG=v1.3.5
VORBIS_TAG=v1.3.7

SRC="$ROOT/build/codecs/src"
BUILD="$ROOT/build/codecs/build-$ABI"
mkdir -p "$SRC" "$BUILD" "$PREFIX"
JOBS="$(nproc 2>/dev/null || echo 4)"

fetch() { # dir url ref
  local dir="$SRC/$1"
  [ "$(cat "$dir/.chippy-ref" 2>/dev/null)" = "$3" ] && return
  rm -rf "$dir"
  git init -q "$dir"
  git -C "$dir" fetch -q --depth 1 "$2" "$3"
  git -C "$dir" checkout -q FETCH_HEAD
  echo "$3" > "$dir/.chippy-ref"
}

cmake_build() { # name srcdir args...
  local name="$1" src="$2"; shift 2
  local cross=()
  if [ "$ABI" != linux ]; then
    cross=(-DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake"
      -DANDROID_ABI="$ABI" -DANDROID_PLATFORM="android-$API")
  fi
  cmake -S "$src" -B "$BUILD/$name" -G "Unix Makefiles" "${cross[@]}" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib \
    -DCMAKE_PREFIX_PATH="$PREFIX" -DCMAKE_FIND_ROOT_PATH="$PREFIX" \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DBUILD_SHARED_LIBS=OFF "$@"
  cmake --build "$BUILD/$name" -j "$JOBS"
  cmake --install "$BUILD/$name"
}

echo "== libogg"
fetch ogg https://github.com/xiph/ogg "refs/tags/$OGG_TAG"
cmake_build ogg "$SRC/ogg" -DINSTALL_DOCS=OFF -DBUILD_TESTING=OFF

echo "== libvorbis"
fetch vorbis https://github.com/xiph/vorbis "refs/tags/$VORBIS_TAG"
cmake_build vorbis "$SRC/vorbis" -DOGG_ROOT="$PREFIX" -DOGG_INCLUDE_DIR="$PREFIX/include" \
  -DOGG_LIBRARY="$PREFIX/lib/libogg.a"

echo "== mpg123"
fetch mpg123 https://github.com/madebr/mpg123 "$MPG123_COMMIT"
cmake_build mpg123 "$SRC/mpg123/ports/cmake" -DBUILD_PROGRAMS=OFF -DBUILD_LIBOUT123=OFF \
  -DBUILD_LIBSYN123=OFF -DCMAKE_POLICY_VERSION_MINIMUM=3.5

echo "== FFmpeg"
fetch ffmpeg https://github.com/FFmpeg/FFmpeg "refs/tags/$FFMPEG_TAG"
mkdir -p "$BUILD/ffmpeg"
(
  cd "$BUILD/ffmpeg"
  if [ "$ABI" = linux ]; then
    # Nothing from the system: the binary must not need it at run time.
    platform=(--disable-autodetect --disable-x86asm)
  else
    platform=(--target-os=android --arch="$FFARCH" --enable-cross-compile
      --cc="$TC/bin/$TRIPLE$API-clang" --cxx="$TC/bin/$TRIPLE$API-clang++"
      --ar="$TC/bin/llvm-ar" --ranlib="$TC/bin/llvm-ranlib" --nm="$TC/bin/llvm-nm"
      --strip="$TC/bin/llvm-strip" --sysroot="$TC/sysroot"
      --disable-jni --disable-mediacodec)
  fi
  "$SRC/ffmpeg/configure" \
    --prefix="$PREFIX" "${platform[@]}" \
    --enable-pic --enable-static --disable-shared \
    --enable-gpl --disable-version3 \
    --disable-programs --disable-doc --disable-avdevice --disable-swscale \
    --disable-avfilter --disable-network --disable-everything --disable-iconv \
    --disable-zlib --disable-vulkan \
    --disable-debug \
    --enable-parser=ac3,mpegaudio,xma,vorbis,opus \
    --enable-demuxer=ac3,eac3,spdif,asf,xwma,mov,oma,xmv,ogg,flac,wav,aac,mp3,smacker,bink,caf,mpc,mpc8,tak,ape,flv,voc \
    --enable-decoder=ac3,eac3,wmapro,wmav1,wmav2,xma1,xma2,aac,atrac3,atrac3p,mp2float,mp3float,smackaud,binkaudio_dct,binkaudio_rdft,pcm_s16be,pcm_s16be_planar,pcm_s16le,pcm_s16le_planar,pcm_s8,pcm_s8_planar,pcm_u8,flac,vorbis,mpc7,mpc8,alac,adpcm_ima_qt,adpcm_ima_dk3,adpcm_ima_dk4,tak,ape,adpcm_swf,opus \
    --enable-swresample \
    --pkg-config-flags=--static
  make -j "$JOBS"
  make install
)

echo "built into $PREFIX:"
ls "$PREFIX/lib"
