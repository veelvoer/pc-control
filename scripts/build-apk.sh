#!/usr/bin/env bash
# Builds a signed release APK. Keystore lives outside the repo (~/.android/pccontrol.keystore).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_ROOT="${ANDROID_NDK_ROOT:-$(ls -d "$ANDROID_HOME"/ndk/* | tail -1)}"
export JAVA_HOME="${JAVA_HOME:-/usr/lib/jvm/java-17-openjdk}"
KS="$HOME/.android/pccontrol.keystore"
if [ ! -f "$KS" ]; then
  mkdir -p "$HOME/.android"
  PW=$(head -c 18 /dev/urandom | base64 | tr -dc A-Za-z0-9)
  "$JAVA_HOME/bin/keytool" -genkeypair -keystore "$KS" -storepass "$PW" -keypass "$PW" -alias pccontrol \
    -keyalg RSA -keysize 2048 -validity 36500 -dname "CN=PC Control" >/dev/null
  echo "$PW" > "$HOME/.android/pccontrol.keystore.pass"; chmod 600 "$HOME/.android/pccontrol.keystore.pass" "$KS"
fi
export CARGO_APK_RELEASE_KEYSTORE="$KS"
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD="$(cat "$HOME/.android/pccontrol.keystore.pass")"
cargo apk build --lib --release -p pccontrol
mkdir -p dist
cp target/release/apk/pc-control.apk dist/pc-control.apk
ls -la dist/pc-control.apk
