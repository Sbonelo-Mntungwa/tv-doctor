#!/bin/sh
# Build TV Nurse, the companion app that runs on the TV, into
# android/helper/build/tv-nurse.apk.
#
# No Gradle: the Android build tools are called directly, which is all a
# four-class app needs. One-off setup on a Mac:
#
#   brew install --cask android-commandlinetools
#   sdkmanager --sdk_root=/opt/homebrew/share/android-commandlinetools \
#       "build-tools;34.0.0" "platforms;android-34"
#
# The app is signed with a key kept in ~/.config/tvdoctor, outside the project.
# Android only accepts an update signed with the same key, so keep that file.
set -e
cd "$(dirname "$0")/../android/helper"

SDK="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
TOOLS="$SDK/build-tools/34.0.0"
PLATFORM="$SDK/platforms/android-34/android.jar"
JAVA_HOME="${JAVA_HOME:-$(/usr/libexec/java_home -v 17)}"
export JAVA_HOME
KEYSTORE="$HOME/.config/tvdoctor/helper-signing.jks"
KEYPASS="tvdoctor-helper"

VERSION_NAME=$(sed -n 's/.*VERSION = "\(.*\)";/\1/p' src/com/tvdoctor/helper/Info.java)
VERSION_CODE=$(sed -n 's/.*VERSION_CODE = \([0-9]*\);/\1/p' src/com/tvdoctor/helper/Info.java)

for need in "$TOOLS/aapt2" "$TOOLS/d8" "$TOOLS/zipalign" "$TOOLS/apksigner" "$PLATFORM"; do
    if [ ! -e "$need" ]; then
        echo "missing $need. See the setup at the top of this script." >&2
        exit 1
    fi
done

rm -rf build
mkdir -p build/gen build/classes build/dex

"$TOOLS/aapt2" compile --dir res -o build/res.zip
"$TOOLS/aapt2" link -o build/unsigned.apk -I "$PLATFORM" \
    --manifest AndroidManifest.xml --java build/gen build/res.zip \
    --min-sdk-version 23 --target-sdk-version 28 \
    --version-code "$VERSION_CODE" --version-name "$VERSION_NAME"

# Plain Java 8 with no lambdas: android.jar has no LambdaMetafactory to
# compile them against.
if ! "$JAVA_HOME/bin/javac" -nowarn -Xlint:-options -source 1.8 -target 1.8 \
    -bootclasspath "$PLATFORM" -d build/classes \
    $(find src build/gen -name '*.java') > build/javac.log 2>&1; then
    cat build/javac.log >&2
    echo "javac failed" >&2
    exit 1
fi

"$TOOLS/d8" --min-api 23 --lib "$PLATFORM" --output build/dex $(find build/classes -name '*.class')
(cd build/dex && zip -q -u ../unsigned.apk classes.dex)

"$TOOLS/zipalign" -f -p 4 build/unsigned.apk build/aligned.apk

if [ ! -f "$KEYSTORE" ]; then
    mkdir -p "$(dirname "$KEYSTORE")"
    "$JAVA_HOME/bin/keytool" -genkeypair -keystore "$KEYSTORE" -alias helper \
        -keyalg RSA -keysize 2048 -validity 10000 \
        -storepass "$KEYPASS" -keypass "$KEYPASS" \
        -dname "CN=TV Nurse" >/dev/null 2>&1
    chmod 600 "$KEYSTORE"
fi

"$TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$KEYPASS" \
    --out build/tv-nurse.apk build/aligned.apk

echo
echo "  Built android/helper/build/tv-nurse.apk (version $VERSION_NAME)"
echo
