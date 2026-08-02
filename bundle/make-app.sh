#!/bin/sh
# Builds the release binary and assembles dist/TigridenR.app (macOS).
set -e
cd "$(dirname "$0")/.."

cargo build --release

APP=dist/TigridenR.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp bundle/Info.plist "$APP/Contents/"
cp target/release/tigridenr "$APP/Contents/MacOS/"
cp assets/icon.icns "$APP/Contents/Resources/"

echo "Built $APP — drag it into /Applications if you like."
