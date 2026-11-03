#!/bin/sh
# Removes what install.sh put in place (your downloads and settings are kept).
set -eu
prefix=${PREFIX:-$HOME/.local}
lib="$prefix/lib/sdm"
[ -x "$lib/sdm-host" ] && "$lib/sdm-host" --uninstall || true
rm -f "$prefix/bin/sdm" "$prefix/share/applications/sdm.desktop"
for s in 16 32 48 64 128 256 512; do rm -f "$prefix/share/icons/hicolor/${s}x${s}/apps/sdm.png"; done
rm -f "$prefix/share/icons/hicolor/scalable/apps/sdm.svg"
rm -rf "$lib"
echo "SDM removed. Settings and history stay in ~/.config/sdm (delete that folder to reset)."
