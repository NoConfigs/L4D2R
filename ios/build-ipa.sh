#!/bin/sh
# Builds an unsigned (ad-hoc signed) L4D2Rewrite.ipa on macOS with the Xcode
# command-line tools. Sideload it with your Apple ID (Sideloadly, AltStore),
# which re-signs it. Used by .github/workflows/ios.yml.
#   ios/build-ipa.sh            -> artifacts/ios/L4D2Rewrite.ipa
set -eu
cd "$(dirname "$0")"
ROOT=$(cd .. && pwd)
TARGET=aarch64-apple-ios
export IPHONEOS_DEPLOYMENT_TARGET=15.0
rustup target add "$TARGET"
SDK=$(xcrun --sdk iphoneos --show-sdk-path)

echo "== Building the Bink-only FFmpeg (menu background video)"
sh build-ffmpeg.sh
FF="$(pwd)/ffmpeg"

echo "== Building the game library ($TARGET)"
cargo build --locked --release --target "$TARGET"
LIB="target/$TARGET/release/libl4d2r_ios.a"

echo "== Linking the app"
BUILD="$ROOT/ios/build"
APP="$BUILD/Payload/L4D2Rewrite.app"
rm -rf "$BUILD"
mkdir -p "$APP"
xcrun --sdk iphoneos clang -arch arm64 -isysroot "$SDK" -miphoneos-version-min=15.0 \
    -fobjc-arc main.m installer.m bink.c -I"$FF/include" "$LIB" "$FF/lib/libavformat.a" "$FF/lib/libavcodec.a" "$FF/lib/libavutil.a" -lm -o "$APP/L4D2Rewrite" \
    -lc++ -lresolv -liconv -Wl,-dead_strip \
    -framework Foundation -framework UIKit -framework QuartzCore -framework Metal \
    -framework CoreGraphics -framework CoreFoundation -framework CoreAudio \
    -framework AudioToolbox -framework AVFoundation -framework GameController \
    -framework CoreHaptics -framework Security -framework SystemConfiguration \
    -framework CoreMotion -framework IOKit -framework CoreText -framework CoreImage \
    -framework CoreMedia -framework CoreVideo

echo "== Assembling the bundle"
cp Info.plist "$APP/Info.plist"
echo "== Compiling the launcher artwork as the iPhone/iPad app icon"
python3 "$ROOT/tools/verify_ios_icon.py"
xcrun --sdk iphoneos actool Assets.xcassets --compile "$APP" \
    --platform iphoneos --minimum-deployment-target 15.0 \
    --target-device iphone --target-device ipad --app-icon AppIcon \
    --output-partial-info-plist "$BUILD/AppIcon.plist"
python3 - "$APP/Info.plist" "$BUILD/AppIcon.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as f:
    info = plistlib.load(f)
with open(sys.argv[2], 'rb') as f:
    info.update(plistlib.load(f))
assert info['CFBundleIcons']['CFBundlePrimaryIcon']['CFBundleIconName'] == 'AppIcon'
with open(sys.argv[1], 'wb') as f:
    plistlib.dump(info, f, sort_keys=False)
PY
cp -R "$ROOT/assets" "$APP/assets"
mkdir -p "$APP/maps"
cp "$ROOT"/maps/rewrite_lab.bsp "$ROOT"/maps/rewrite_lab.json "$APP/maps/"
# The .nav starts with 0xFEEDFACE (the Mach-O magic), which makes sideload
# signers reject the IPA; ship it behind a 4-byte header (src/test_lab.rs).
{ printf 'L4DR'; cat "$ROOT/maps/rewrite_lab.nav"; } > "$APP/maps/rewrite_lab.navpack"
# Fail the build if any other bundled data file looks like a Mach-O.
for f in $(find "$APP" -type f ! -name L4D2Rewrite); do
    case "$(head -c 4 "$f" | od -An -tx1 | tr -d ' ')" in
        cefaedfe|cffaedfe|feedface|feedfacf|cafebabe) echo "Mach-O magic in $f"; exit 1 ;;
    esac
done
printf 'APPL????' > "$APP/PkgInfo"
# Ad-hoc signature; sideloading tools replace it with your Apple ID's.
codesign --force --sign - --timestamp=none "$APP"

OUT="$ROOT/artifacts/ios"
mkdir -p "$OUT"
rm -f "$OUT/L4D2Rewrite.ipa"
(cd "$BUILD" && zip -qry "$OUT/L4D2Rewrite.ipa" Payload)
echo "== IPA: $OUT/L4D2Rewrite.ipa ($(du -h "$OUT/L4D2Rewrite.ipa" | cut -f1))"
