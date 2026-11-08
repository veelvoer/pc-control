#!/usr/bin/env bash
# Builds dist/PC-Control-x86_64.AppImage (host daemon + desktop client).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p pccontrol-host -p pccontrol
A="$ROOT/target/AppDir"; rm -rf "$A"; mkdir -p "$A/usr/bin" "$A/usr/share/pc-control" dist
cp target/release/pccontrol-host target/release/pccontrol "$A/usr/bin/"
cp packaging/install.sh packaging/icon.png packaging/pc-control.desktop "$A/usr/share/pc-control/"
cp packaging/icon.png "$A/pc-control.png"; cp packaging/pc-control.desktop "$A/pc-control.desktop"
cat > "$A/AppRun" <<'RUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
case "${1:-}" in
  install) exec "$HERE/usr/share/pc-control/install.sh" "$HERE/usr/bin" ;;
  host) shift; exec "$HERE/usr/bin/pccontrol-host" "$@" ;;
  run|pair|reset-token|-h|--help) exec "$HERE/usr/bin/pccontrol-host" "$@" ;;
  *) exec "$HERE/usr/bin/pccontrol" "$@" ;;
esac
RUN
chmod +x "$A/AppRun"
TOOL="${APPIMAGETOOL:-}"
if [ -z "$TOOL" ]; then
  TD="$(mktemp -d)"
  curl -fsSL -o "$TD/appimagetool" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$TD/appimagetool"; TOOL="$TD/appimagetool"
fi
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$A" "$ROOT/dist/PC-Control-x86_64.AppImage"
ls -la dist/PC-Control-x86_64.AppImage
