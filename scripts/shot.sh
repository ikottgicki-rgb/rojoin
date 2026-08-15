#!/usr/bin/env bash
# Render the app in an isolated headless compositor and capture a PNG.
#
# Nothing here touches the user's real desktop: sway runs on the wlroots
# headless backend inside a private XDG_RUNTIME_DIR, so it has its own Wayland
# socket and its own outputs.
#
# XDG_CONFIG_HOME is forced to a throwaway directory too. A v1 test once wrote
# into the user's real config; that must never happen again.
#
# Usage: scripts/shot.sh <out.png> [width] [height]
#   ROJOIN_DEMO=1 ROJOIN_SECTION=N ROJOIN_VIEW=1  which screen to open
set -euo pipefail

cd "$(dirname "$0")/.."

OUT="${1:-/tmp/rojoin-shot.png}"
W="${2:-1400}"
H="${3:-900}"
BIN="${BIN:-target/debug/rojoin-v4}"

[ -x "$BIN" ] || { echo "no binary at $BIN — run cargo build first"; exit 1; }

RT=$(mktemp -d /tmp/rojoin-rt.XXXXXX)
CFGDIR=$(mktemp -d /tmp/rojoin-cfg.XXXXXX)
chmod 700 "$RT"
SWAYCFG="$RT/sway.conf"

cleanup() {
    [ -n "${SWAYPID:-}" ] && kill "$SWAYPID" 2>/dev/null || true
    sleep 0.3
    rm -rf "$RT" "$CFGDIR"
}
trap cleanup EXIT

cat > "$SWAYCFG" <<EOF
output HEADLESS-1 resolution ${W}x${H}
default_border none
default_floating_border none
exec "$PWD/$BIN"
EOF

XDG_RUNTIME_DIR="$RT" \
XDG_CONFIG_HOME="$CFGDIR" \
WLR_BACKENDS=headless \
WLR_LIBINPUT_NO_DEVICES=1 \
LIBGL_ALWAYS_SOFTWARE=1 \
ROJOIN_DEMO="${ROJOIN_DEMO:-0}" \
ROJOIN_SECTION="${ROJOIN_SECTION:-0}" \
ROJOIN_VIEW="${ROJOIN_VIEW:-0}" \
    sway -c "$SWAYCFG" >"$RT/sway.log" 2>&1 &
SWAYPID=$!

# Wait for the compositor socket.
for _ in $(seq 1 60); do
    WD=$(find "$RT" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' -printf '%f\n' 2>/dev/null | head -1)
    [ -n "$WD" ] && break
    sleep 0.25
done
[ -n "${WD:-}" ] || { echo "compositor never came up:"; cat "$RT/sway.log"; exit 1; }

# Give the app time to map a window and paint its first frame.
sleep 4

XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" grim "$OUT"
echo "wrote $OUT ($(identify -format '%wx%h' "$OUT" 2>/dev/null || echo '?'))"
