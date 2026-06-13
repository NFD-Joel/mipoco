#!/usr/bin/env bash
# Build a portable mipoco AppImage (x86_64).
# Assumes `cargo build --release` has produced target/release/mipoco.
# appimagetool is downloaded once into packaging/.tools/.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

ARCH=x86_64
VERSION="$(awk -F'\"' '/^version =/{print $2; exit}' Cargo.toml)"
BIN=target/release/mipoco
TOOLS=packaging/.tools
APPDIR=target/appimage/mipoco.AppDir
OUT="target/mipoco-${VERSION}-${ARCH}.AppImage"

[ -x "$BIN" ] || { echo "build first: cargo build --release" >&2; exit 1; }

# --- fetch appimagetool ---------------------------------------------------
mkdir -p "$TOOLS"
TOOL="$TOOLS/appimagetool-${ARCH}.AppImage"
if [ ! -x "$TOOL" ]; then
    echo "downloading appimagetool..."
    curl -fsSL -o "$TOOL" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage"
    chmod +x "$TOOL"
fi

# --- assemble AppDir ------------------------------------------------------
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps"
install -m755 "$BIN" "$APPDIR/usr/bin/mipoco"
install -m755 packaging/linux/mipoco-launcher "$APPDIR/usr/bin/mipoco-launcher"
install -m644 packaging/linux/mipoco.desktop "$APPDIR/mipoco.desktop"
install -m644 packaging/linux/mipoco.desktop "$APPDIR/usr/share/applications/mipoco.desktop"
install -m644 packaging/linux/icons/hicolor/256x256/apps/mipoco.png "$APPDIR/mipoco.png"
install -m644 packaging/linux/icons/hicolor/256x256/apps/mipoco.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/mipoco.png"
cp "$APPDIR/mipoco.png" "$APPDIR/.DirIcon"

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
# absolute path so the launcher (and any terminal it spawns) always finds it
export MIPOCO_BIN="$HERE/usr/bin/mipoco"
exec "$HERE/usr/bin/mipoco-launcher" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# --- package --------------------------------------------------------------
# extract-and-run avoids needing FUSE for appimagetool itself.
ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" "$APPDIR" "$OUT"
echo "built $OUT"
