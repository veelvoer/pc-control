#!/usr/bin/env bash
# Installs PC Control for the current user. Usage: install.sh <dir containing pccontrol-host and pccontrol>
set -euo pipefail
SRC="${1:?usage: install.sh <bin dir>}"
HERE="$(dirname "$(readlink -f "$0")")"
BIN="$HOME/.local/bin"
install -Dm755 "$SRC/pccontrol-host" "$BIN/pccontrol-host"
install -Dm755 "$SRC/pccontrol" "$BIN/pccontrol"
install -Dm644 "$HERE/icon.png" "$HOME/.local/share/icons/hicolor/256x256/apps/pc-control.png"
sed "s#^Exec=.*#Exec=$BIN/pccontrol#" "$HERE/pc-control.desktop" > /tmp/pc-control.desktop.$$
install -Dm644 /tmp/pc-control.desktop.$$ "$HOME/.local/share/applications/pc-control.desktop"; rm -f /tmp/pc-control.desktop.$$
mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/pccontrol-host.service" <<UNIT
[Unit]
Description=PC Control host (remote screen, input and stats)
After=graphical-session.target

[Service]
ExecStart=$BIN/pccontrol-host run
Restart=on-failure
RestartSec=3
Nice=5

[Install]
WantedBy=default.target
UNIT
systemctl --user daemon-reload
systemctl --user enable --now pccontrol-host.service
sleep 1
echo "Installed. Host is running as a user service (journalctl --user -u pccontrol-host)."
echo "Port 7878/tcp must be reachable from your phone."
echo "Pairing code (copy it into the phone app):"
"$BIN/pccontrol-host" pair
