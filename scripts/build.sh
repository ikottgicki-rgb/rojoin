#!/usr/bin/env bash
# Build RoJoin v4 for both targets and assert the Windows artifact really is a
# single self-contained .exe. Run this every milestone, not just at the end —
# a Windows break found late is a Windows break that is expensive to unpick.
set -euo pipefail

cd "$(dirname "$0")/.."

LINUX_TARGET=x86_64-unknown-linux-gnu
WIN_TARGET=x86_64-pc-windows-gnu

mode="${1:-release}"
flag=""
[ "$mode" = "release" ] && flag="--release"

echo "==> Linux ($mode)"
cargo build $flag --target "$LINUX_TARGET"

echo "==> Windows ($mode)"
cargo build $flag --target "$WIN_TARGET"

exe="target/$WIN_TARGET/$mode/rojoin-v4.exe"

echo
echo "==> Windows artifact checks"
ls -lh "$exe"

echo
echo "-- DLL imports (must be Windows system libs only; any libgcc/libwinpthread/libstdc++ is a FAILURE) --"
imports=$(objdump -p "$exe" | grep 'DLL Name:' | sed 's/.*DLL Name: //' | sort -u)
echo "$imports" | sed 's/^/   /'

if echo "$imports" | grep -qiE 'libgcc|libwinpthread|libstdc\+\+'; then
    echo
    echo "FAIL: the exe depends on mingw runtime DLLs — it is not self-contained."
    exit 1
fi

echo
echo "-- Subsystem (must be GUI, not console) --"
# Anchor the match: objdump also prints Major/MinorSubsystemVersion, and a
# loose grep picks those up instead of the field we actually care about.
subsystem=$(objdump -p "$exe" | grep -E '^Subsystem')
echo "   $subsystem"
if [ "$mode" = "release" ] && ! echo "$subsystem" | grep -qi 'GUI'; then
    echo
    echo "FAIL: release exe is a console subsystem binary — it will flash a black window."
    echo "      Check that crates/app/src/main.rs still opens with windows_subsystem = \"windows\"."
    exit 1
fi

echo
echo "OK: single self-contained GUI .exe"
