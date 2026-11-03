#!/bin/sh
# Installs SDM for the current user (no root needed):
#   binaries    -> ~/.local/lib/sdm   (symlinked into ~/.local/bin)
#   app grid    -> ~/.local/share/applications/sdm.desktop + icons
# Usage: cargo build --release && packaging/install.sh      (PREFIX=/usr/local for system-wide)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
src=${SDM_BIN_DIR:-$root/target/release}
prefix=${PREFIX:-$HOME/.local}
lib="$prefix/lib/sdm"

for b in sdm sdm-host; do
  [ -x "$src/$b" ] || { echo "Missing $src/$b - run: cargo build --release" >&2; exit 1; }
done

mkdir -p "$lib" "$prefix/bin" "$prefix/share/applications"
install -m 755 "$src/sdm" "$src/sdm-host" "$lib/"
[ -x "$src/sdm-cli" ] && install -m 755 "$src/sdm-cli" "$lib/"
ln -sf "$lib/sdm" "$prefix/bin/sdm"
[ -x "$lib/sdm-cli" ] && ln -sf "$lib/sdm-cli" "$prefix/bin/sdm-cli"

for s in 16 32 48 64 128 256 512; do
  d="$prefix/share/icons/hicolor/${s}x${s}/apps"
  mkdir -p "$d"
  install -m 644 "$here/icons/sdm-$s.png" "$d/sdm.png"
done
mkdir -p "$prefix/share/icons/hicolor/scalable/apps"
install -m 644 "$here/icons/sdm.svg" "$prefix/share/icons/hicolor/scalable/apps/sdm.svg"
# Absolute Exec so the launcher works even when ~/.local/bin is not on the desktop's PATH.
sed "s|^Exec=.*|Exec=$lib/sdm %U|" "$here/sdm.desktop" > "$prefix/share/applications/sdm.desktop"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache  >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true

echo "SDM installed. Find it in your app grid, or run: $prefix/bin/sdm"
echo "Browser extension: open SDM -> Settings -> Set up browser extension."
