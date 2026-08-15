#!/usr/bin/env bash
# Headless smoke test: launch the app in an isolated compositor, let it settle,
# and fail on any panic, Slint error, or RefCell double-borrow.
#
# The RefCell check is specific and deliberate: applying an image to a model
# synchronously from a repeater delegate's init panics with "already borrowed",
# and it only reproduces once real thumbnails start arriving. Catching it here
# is much cheaper than catching it from a user report.
#
# As with shot.sh, XDG_CONFIG_HOME is redirected to a throwaway directory so a
# test run can never touch the real config.
set -euo pipefail

cd "$(dirname "$0")/.."

BIN="${BIN:-target/debug/rojoin-v4}"
SETTLE="${SETTLE:-8}"
W=1400
H=900

[ -x "$BIN" ] || { echo "no binary at $BIN — run cargo build first"; exit 1; }

RT=$(mktemp -d /tmp/rojoin-smoke-rt.XXXXXX)
CFGDIR=$(mktemp -d /tmp/rojoin-smoke-cfg.XXXXXX)
chmod 700 "$RT"
LOG="$RT/app.log"

cleanup() {
    [ -n "${SWAYPID:-}" ] && kill "$SWAYPID" 2>/dev/null || true
    sleep 0.3
    rm -rf "$RT" "$CFGDIR"
}
trap cleanup EXIT

cat > "$RT/sway.conf" <<EOF
output HEADLESS-1 resolution ${W}x${H}
default_border none
exec "$PWD/$BIN" 2>"$LOG"
EOF

XDG_RUNTIME_DIR="$RT" \
XDG_CONFIG_HOME="$CFGDIR" \
WLR_BACKENDS=headless \
WLR_LIBINPUT_NO_DEVICES=1 \
LIBGL_ALWAYS_SOFTWARE=1 \
RUST_BACKTRACE=1 \
    sway -c "$RT/sway.conf" >"$RT/sway.log" 2>&1 &
SWAYPID=$!

for _ in $(seq 1 60); do
    WD=$(find "$RT" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' -printf '%f\n' 2>/dev/null | head -1)
    [ -n "$WD" ] && break
    sleep 0.25
done
[ -n "${WD:-}" ] || { echo "FAIL: compositor never came up"; cat "$RT/sway.log"; exit 1; }

sleep "$SETTLE"

# Did the process survive?
if ! pgrep -f "$(basename "$BIN")" >/dev/null; then
    echo "FAIL: the app exited during the settle window"
    [ -f "$LOG" ] && cat "$LOG"
    exit 1
fi

status=0
if [ -f "$LOG" ]; then
    if grep -qE "panicked at|already borrowed|BorrowMutError" "$LOG"; then
        echo "FAIL: panic detected"
        grep -nE "panicked at|already borrowed|BorrowMutError" -A 6 "$LOG"
        status=1
    fi
    if grep -qiE "slint.*error|Cannot access id" "$LOG"; then
        echo "FAIL: Slint runtime error"
        grep -niE "slint.*error|Cannot access id" -A 3 "$LOG"
        status=1
    fi
fi

if [ "$status" -eq 0 ]; then
    echo "OK: ran ${SETTLE}s headless with no panics or Slint errors"
    [ -f "$LOG" ] && [ -s "$LOG" ] && { echo "--- app log ---"; cat "$LOG"; }
fi

exit "$status"
