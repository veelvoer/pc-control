#!/usr/bin/env bash
# Dev aid (Hyprland): launch the client floating at a given size, screenshot it, close it.
# usage: BIN=path scripts/dev-shot.sh <out.png> <WxH> [VAR=value ...]
set -euo pipefail
OUT="$1"; SIZE="$2"; shift 2
W="${SIZE%x*}"; H="${SIZE#*x}"
BIN="${BIN:-$(dirname "$0")/../target/release/pccontrol}"
pkill -x pccontrol 2>/dev/null || true; sleep 0.5
env "$@" "$BIN" >/dev/null 2>&1 &
for _ in $(seq 40); do hyprctl clients -j | grep -q '"class": "pccontrol"' && break; sleep 0.25; done
X=$(( (1920 - W) / 2 )); Y=$(( (1080 - H) / 2 )); [ "$Y" -lt 0 ] && Y=0
hyprctl dispatch 'hl.dsp.window.float({ action = "enable", window = "class:pccontrol" })' >/dev/null
hyprctl dispatch "hl.dsp.window.resize({ x = $W, y = $H, exact = true, window = \"class:pccontrol\" })" >/dev/null
hyprctl dispatch "hl.dsp.window.move({ x = $X, y = $Y, exact = true, window = \"class:pccontrol\" })" >/dev/null
sleep "${WAIT:-4}"
grim -g "$X,$Y ${W}x${H}" "$OUT"
pkill -x pccontrol || true
