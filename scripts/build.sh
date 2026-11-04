#!/bin/sh
# Build TV Doctor and install it as TVDoctor.app in ~/Applications.
#
# The app is put together from macos/ each time: Info.plist, the icon, the
# Finder launcher, and the freshly built binary, then ad-hoc signed. The binary
# has to live inside the app so macOS gives it the app's Local Network
# permission, and the signature has to be redone after every copy or Finder
# will not open it.
#
# Afterwards open it from anywhere with:  open -a TVDoctor
# or find "TV Doctor" in Spotlight or Launchpad.
set -e
cd "$(dirname "$0")/.."

cargo build --release

app="$HOME/Applications/TVDoctor.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp -f macos/Info.plist "$app/Contents/Info.plist"
cp -f macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cp -f macos/launch "$app/Contents/MacOS/launch"
cp -f target/release/tvdoctor "$app/Contents/MacOS/tvdoctor"
chmod +x "$app/Contents/MacOS/launch" "$app/Contents/MacOS/tvdoctor"
codesign --force --deep --sign - "$app"
# Tell Launch Services about the new copy, so Spotlight and the icon update.
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" 2>/dev/null || true

echo
echo "  Installed to ~/Applications/TVDoctor.app"
echo "  Open it with:   open -a TVDoctor     (or Spotlight: TV Doctor)"
echo
