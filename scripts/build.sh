#!/bin/sh
# Build TV Doctor and assemble the macOS app in dist/TV Doctor.app.
#
# The app is put together from macos/ each time: Info.plist, the Finder
# launcher, and the freshly built binary, then ad-hoc signed. The binary has to
# live inside the app so macOS gives it the app's Local Network permission, and
# the signature has to be redone after every copy or Finder will not open it.
set -e
cd "$(dirname "$0")/.."

cargo build --release

app="dist/TV Doctor.app"
mkdir -p "$app/Contents/MacOS"
cp -f macos/Info.plist "$app/Contents/Info.plist"
cp -f macos/launch "$app/Contents/MacOS/launch"
cp -f target/release/tvdoctor "$app/Contents/MacOS/tvdoctor"
chmod +x "$app/Contents/MacOS/launch" "$app/Contents/MacOS/tvdoctor"
codesign --force --deep --sign - "$app"

echo
echo "  Built. Start it with:   open \"$app\""
echo "  Or from the terminal:   ./target/release/tvdoctor"
echo
