#!/usr/bin/env bash
# Produce the two shipping artifacts:
#   dist/RoJoin-x86_64.AppImage   — single-file Linux build
#   dist/RoJoin-windows-x64.zip   — the standalone .exe plus a README
#
# Runs scripts/build.sh first, so the Windows self-contained and no-console
# assertions gate packaging too.
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
LINUX_BIN=target/x86_64-unknown-linux-gnu/release/rojoin-v4
WIN_BIN=target/x86_64-pc-windows-gnu/release/rojoin-v4.exe

echo "==> Building both targets (version $VERSION)"
./scripts/build.sh release

mkdir -p dist

# ---------------------------------------------------------------- Windows ---
echo
echo "==> Windows zip"
rm -rf dist/windows
mkdir -p dist/windows
cp "$WIN_BIN" dist/windows/RoJoin.exe

cat > dist/windows/README.txt <<EOF
RoJoin $VERSION — Windows

Run RoJoin.exe. There is nothing to install and no other files are needed.

Sign-in happens in your own browser: RoJoin shows a code, you approve it on
roblox.com or in the Roblox mobile app. No password is ever typed into RoJoin.
EOF

(cd dist/windows && zip -q -r "../RoJoin-windows-x64.zip" .)
echo "    dist/RoJoin-windows-x64.zip ($(du -h dist/RoJoin-windows-x64.zip | cut -f1))"

# ------------------------------------------------------------------ Linux ---
echo
echo "==> Linux AppImage"

if ! command -v appimagetool >/dev/null 2>&1; then
    cat <<'EOF'
    SKIPPED: appimagetool is not installed.

    Install it with one of:
      paru -S appimagetool-bin
      # or download the AppImage from
      # https://github.com/AppImage/AppImageKit/releases

    The Linux binary is still built and runnable at
    target/x86_64-unknown-linux-gnu/release/rojoin-v4
EOF
    exit 0
fi

APPDIR=dist/AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps"

cp "$LINUX_BIN" "$APPDIR/usr/bin/rojoin-v4"

cat > "$APPDIR/rojoin-v4.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=RoJoin
Comment=Native Roblox client
Exec=rojoin-v4 %u
Icon=rojoin-v4
Categories=Game;
Terminal=false
MimeType=x-scheme-handler/roblox;
EOF
cp "$APPDIR/rojoin-v4.desktop" "$APPDIR/usr/share/applications/"

# A generated icon keeps packaging self-contained; replace assets/icon.png to
# ship a real one.
if [ -f assets/icon.png ]; then
    cp assets/icon.png "$APPDIR/rojoin-v4.png"
else
    convert -size 256x256 xc:'#000000' \
        -fill '#4C8DFF' -draw 'rectangle 40,40 216,216' \
        -fill '#000000' -draw 'rectangle 72,72 184,184' \
        "$APPDIR/rojoin-v4.png" 2>/dev/null || \
        printf '' > "$APPDIR/rojoin-v4.png"
fi
cp "$APPDIR/rojoin-v4.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/" 2>/dev/null || true

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/rojoin-v4" "$@"
EOF
chmod +x "$APPDIR/AppRun"

APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 appimagetool "$APPDIR" dist/RoJoin-x86_64.AppImage
echo "    dist/RoJoin-x86_64.AppImage ($(du -h dist/RoJoin-x86_64.AppImage | cut -f1))"

echo
echo "OK: both artifacts built"
