#!/bin/sh
# Builds an RPM on Fedora:  sudo dnf install rpm-build cargo gcc fontconfig-devel && packaging/make-rpm.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
ver=$(sed -n 's/^Version:[[:space:]]*//p' "$root/packaging/sdm.spec")
top=${RPM_TOPDIR:-$HOME/rpmbuild}
mkdir -p "$top/SOURCES" "$top/SPECS"
tmp=$(mktemp -d)
mkdir -p "$tmp/sdm-$ver"
# A normal build compiles from source, so leave target/ out; with SDM_SKIP_BUILD the
# prebuilt binaries in target/release are packaged as they are.
excl=--exclude=./target
[ -n "${SDM_SKIP_BUILD:-}" ] && excl=--exclude=./.nothing
( cd "$root" && tar $excl --exclude=./.git --exclude='./extension/node_modules' -cf - . ) | tar -xf - -C "$tmp/sdm-$ver"
tar -czf "$top/SOURCES/sdm-$ver.tar.gz" -C "$tmp" "sdm-$ver"
cp "$root/packaging/sdm.spec" "$top/SPECS/"
rpmbuild --define "_topdir $top" -ba "$top/SPECS/sdm.spec" ${RPMBUILD_ARGS:-} ${SDM_SKIP_BUILD:+--define 'sdm_skip_build 1'}
echo "RPMs are in $top/RPMS"
