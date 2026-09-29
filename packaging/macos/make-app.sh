#!/usr/bin/env bash
# Builds dist/Lofify.app (and dist/Lofify-<version>-macos.zip).
#
#   packaging/macos/make-app.sh                 # current architecture
#   UNIVERSAL=1 packaging/macos/make-app.sh     # arm64 + x86_64 via lipo
#   CODESIGN_IDENTITY="Developer ID Application: ..." packaging/macos/make-app.sh
#
# The bundle carries Info.plist with NSAudioCaptureUsageDescription, which macOS requires before
# an app may create a system audio tap. Without a Developer ID identity the app is signed ad hoc
# ("-"), which is enough to run it locally and to keep the granted permission across launches.
# UNTESTED: this script has not been run on a Mac.
set -euo pipefail

cd "$(dirname "$0")/../.."
VERSION="$(cargo pkgid -p lofify-app | sed 's/.*[#@]//')"
APP="dist/Lofify.app"
IDENTITY="${CODESIGN_IDENTITY:--}"

build() { cargo build --release -p lofify-app "$@"; }

if [[ "${UNIVERSAL:-0}" == "1" ]]; then
    build --target aarch64-apple-darwin
    build --target x86_64-apple-darwin
    BIN="target/lofify-app-universal"
    lipo -create -output "$BIN" \
        target/aarch64-apple-darwin/release/lofify-app \
        target/x86_64-apple-darwin/release/lofify-app
else
    build
    BIN="target/release/lofify-app"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Lofify"
chmod +x "$APP/Contents/MacOS/Lofify"
sed "s/@VERSION@/${VERSION}/g" packaging/macos/Info.plist.in > "$APP/Contents/Info.plist"
if [[ -f packaging/macos/Lofify.icns ]]; then
    cp packaging/macos/Lofify.icns "$APP/Contents/Resources/Lofify.icns"
fi
cp LICENSE-MIT LICENSE-APACHE "$APP/Contents/Resources/"

codesign --force --deep --sign "$IDENTITY" "$APP"
codesign --verify --verbose "$APP"

ditto -c -k --keepParent "$APP" "dist/Lofify-${VERSION}-macos.zip"
echo "Built $APP and dist/Lofify-${VERSION}-macos.zip"
echo "First launch: macOS will ask for permission to record system audio. Allow it."
