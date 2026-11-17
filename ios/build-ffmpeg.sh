#!/bin/sh
# Builds a minimal static FFmpeg for iOS arm64 holding only what the menu
# background needs: the Bink demuxer and Bink video decoder (ios/bink.c).
# LGPL-only (no --enable-gpl), no programs, no network, no autodetected
# system libraries. Output: ios/ffmpeg/{include,lib}. Used by build-ipa.sh;
# the result is cached by .github/workflows/ios.yml.
set -eu
cd "$(dirname "$0")"
TAG=n7.1.2
COMMIT=f893221c8d89cb798b829bebe71d55e1a3f242fd
OUT="$(pwd)/ffmpeg"
STAMP="$OUT/.built-$COMMIT"
if [ -f "$STAMP" ] && [ -f "$OUT/lib/libavcodec.a" ]; then
    echo "== FFmpeg $TAG already built"
    exit 0
fi
SRC="$(pwd)/ffmpeg-src"
rm -rf "$SRC" "$OUT"
git clone --quiet --depth 1 --branch "$TAG" https://github.com/FFmpeg/FFmpeg.git "$SRC"
GOT=$(git -C "$SRC" rev-parse HEAD)
if [ "$GOT" != "$COMMIT" ]; then
    echo "FFmpeg $TAG is $GOT, expected $COMMIT"
    exit 1
fi
SDK=$(xcrun --sdk iphoneos --show-sdk-path)
CC="$(xcrun --sdk iphoneos -f clang)"
FLAGS="-arch arm64 -isysroot $SDK -miphoneos-version-min=15.0"
cd "$SRC"
./configure --prefix="$OUT" \
    --enable-cross-compile --target-os=darwin --arch=aarch64 \
    --cc="$CC" --sysroot="$SDK" \
    --extra-cflags="$FLAGS" --extra-ldflags="$FLAGS" \
    --enable-static --disable-shared --enable-pic --disable-asm \
    --disable-everything --disable-autodetect --disable-programs --disable-doc \
    --disable-network --disable-avdevice --disable-avfilter --disable-swscale \
    --disable-swresample --disable-postproc \
    --enable-protocol=file --enable-demuxer=bink --enable-decoder=bink
make -j"$(sysctl -n hw.ncpu)"
make install
cd ..
rm -rf "$SRC"
touch "$STAMP"
echo "== FFmpeg $TAG (Bink only): $(du -sh "$OUT/lib" | cut -f1)"
