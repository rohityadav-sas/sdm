#!/bin/sh
# Zips the browser extension for stores / signing:
#   dist/sdm-chromium.zip   (Chrome Web Store, or "Load unpacked" after unzipping)
#   dist/sdm-firefox.zip    (upload to addons.mozilla.org as "unlisted" to get a signed .xpi)
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
ext="$root/extension"
out="$root/dist"
mkdir -p "$out"
for b in chromium firefox; do
  stage=$(mktemp -d)
  cp "$ext/common.js" "$ext/background.js" "$ext/popup.html" "$ext/popup.js" "$stage/"
  cp -r "$ext/icons" "$stage/icons"
  cp "$ext/manifest.$b.json" "$stage/manifest.json"
  rm -f "$out/sdm-$b.zip"
  ( cd "$stage" && zip -qr "$out/sdm-$b.zip" . )
  rm -rf "$stage"
done
echo "Wrote $out/sdm-chromium.zip and $out/sdm-firefox.zip"
echo "Firefox signing: npx web-ext sign --channel=unlisted --source-dir <unzipped firefox folder> --api-key <AMO key> --api-secret <AMO secret>"
