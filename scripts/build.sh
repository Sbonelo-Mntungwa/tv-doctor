#!/bin/sh
# Build TV Doctor and refresh the app bundle.
#
# Run this after changing the source. It builds, copies the binary into
# TV Doctor.app and re-signs the bundle, which Finder needs in order to launch
# it and macOS needs to keep the Local Network permission attached.
set -e
cd "$(dirname "$0")/.."

cargo build --release
cp -f target/release/tvdoctor "TV Doctor.app/Contents/MacOS/tvdoctor"
codesign --force --deep --sign - "TV Doctor.app"

echo
echo "  Built. Start it with:   open \"TV Doctor.app\""
echo "  Or from the terminal:   ./target/release/tvdoctor"
echo
