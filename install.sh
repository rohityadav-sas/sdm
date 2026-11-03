#!/bin/sh
# Smart Download Manager (SDM) installer for Linux and macOS
#   curl -fsSL https://raw.githubusercontent.com/rohityadav-sas/sdm/main/install.sh | sh
#
# Optional environment variables:
#   SDM_VERSION      release tag to install (default: latest), e.g. v1.0.0
#   SDM_INSTALL_DIR  install directory (default: $HOME/.local/bin)
set -eu

REPO="rohityadav-sas/sdm"
BIN="sdm"
INSTALL_DIR="${SDM_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${SDM_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

case "$INSTALL_DIR" in
  /*) ;;
  *) die "SDM_INSTALL_DIR must be an absolute path (got '$INSTALL_DIR')" ;;
esac

# --- platform detection -----------------------------------------------------
case "$(uname -s)" in
  Linux)  os="linux" ;;
  Darwin) os="macos" ;;
  *) die "unsupported OS: $(uname -s). On Windows use: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  arch="x86_64" ;;
  aarch64|arm64) arch="arm64" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
suffix="${os}-${arch}.zip"

# --- downloader -------------------------------------------------------------
if have curl; then
  fetch() { curl -fsSL "$1"; }
  fetch_to() { curl -fsSL "$1" -o "$2"; }
elif have wget; then
  fetch() { wget -qO- "$1"; }
  fetch_to() { wget -qO "$2" "$1"; }
else
  die "curl or wget is required"
fi

# --- find the release asset -------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  api="https://api.github.com/repos/$REPO/releases/latest"
else
  api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
fi

say "Looking up $VERSION release..."
release="$(fetch "$api" 2>/dev/null || true)"
url=""
if [ -n "$release" ]; then
  url="$(printf '%s' "$release" \
    | grep -o "\"browser_download_url\": *\"[^\"]*${suffix}\"" \
    | head -n 1 | cut -d '"' -f 4 || true)"
  if [ -z "$url" ]; then
    url="$(printf '%s' "$release" \
      | grep -o "\"browser_download_url\": *\"[^\"]*${os}[^\"]*\"" \
      | head -n 1 | cut -d '"' -f 4 || true)"
  fi
fi

# Fallback to local source build if release asset is not yet available and cargo exists
if [ -z "$url" ]; then
  if have cargo; then
    say "No prebuilt release asset for '$suffix' was found on GitHub."
    say "Building from source with cargo..."
    tmp_src="$(mktemp -d 2>/dev/null || mktemp -d -t sdm-src)"
    trap 'rm -rf "$tmp_src"' EXIT INT TERM
    git clone --depth 1 "https://github.com/$REPO.git" "$tmp_src" || die "failed to clone repository"
    (cd "$tmp_src" && cargo build --release) || die "build failed"
    mkdir -p "$INSTALL_DIR"
    for b in sdm sdm-host sdm-cli; do
      if [ -f "$tmp_src/target/release/$b" ]; then
        install -m 755 "$tmp_src/target/release/$b" "$INSTALL_DIR/$b"
      fi
    done
    if [ "$os" = "linux" ] && [ -d "$tmp_src/packaging" ]; then
      PREFIX="$(dirname "$INSTALL_DIR")" "$tmp_src/packaging/install.sh" >/dev/null 2>&1 || true
    fi
    say "Installed sdm to $INSTALL_DIR/$BIN"
    say "Run: $BIN"
    exit 0
  else
    die "no prebuilt release asset ending in '$suffix' found and cargo is not installed"
  fi
fi

# --- download and extract ---------------------------------------------------
tmp="$(mktemp -d 2>/dev/null || mktemp -d -t sdm)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading ${url##*/}..."
fetch_to "$url" "$tmp/sdm.zip" || die "download failed"

if have unzip; then
  unzip -q -o "$tmp/sdm.zip" -d "$tmp/out" || die "could not extract the archive"
elif have bsdtar; then
  mkdir -p "$tmp/out" && bsdtar -xf "$tmp/sdm.zip" -C "$tmp/out" || die "could not extract the archive"
elif have tar && printf '%s' "$url" | grep -q '\.tar\.gz$'; then
  mkdir -p "$tmp/out" && tar -xzf "$tmp/sdm.zip" -C "$tmp/out" || die "could not extract the archive"
else
  die "unzip or tar is required to extract the release"
fi

# Locate binary
found="$(find "$tmp/out" -type f -name "$BIN" -perm -111 2>/dev/null | head -n 1 || true)"
if [ -z "$found" ]; then
  found="$(find "$tmp/out" -type f -name "$BIN" 2>/dev/null | head -n 1 || true)"
fi
[ -n "$found" ] || die "'$BIN' was not found inside the archive"

# --- install ----------------------------------------------------------------
mkdir -p "$INSTALL_DIR"
chmod +x "$found"
mv -f "$found" "$INSTALL_DIR/$BIN"

# Also install companion helpers if present in archive
for h in sdm-host sdm-cli; do
  h_found="$(find "$tmp/out" -type f -name "$h" 2>/dev/null | head -n 1 || true)"
  if [ -n "$h_found" ]; then
    chmod +x "$h_found"
    mv -f "$h_found" "$INSTALL_DIR/$h"
  fi
done

# If packaging scripts are in the archive, install desktop entries on Linux
if [ "$os" = "linux" ]; then
  pkg_dir="$(find "$tmp/out" -type d -name "packaging" 2>/dev/null | head -n 1 || true)"
  if [ -n "$pkg_dir" ] && [ -x "$pkg_dir/install.sh" ]; then
    PREFIX="$(dirname "$INSTALL_DIR")" "$pkg_dir/install.sh" >/dev/null 2>&1 || true
  fi
fi

say "Installed $BIN to $INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) say "Run: $BIN" ;;
  *)
    case "$(basename "${SHELL:-}")" in
      zsh)  rc="~/.zshrc" ;;
      bash) if [ "$os" = "macos" ]; then rc="~/.bash_profile"; else rc="~/.bashrc"; fi ;;
      *)    rc="" ;;
    esac
    say ""
    if [ -n "$rc" ]; then
      say "$INSTALL_DIR is not on your PATH. Add this line to $rc, then restart your shell:"
      say "  export PATH=\"$INSTALL_DIR:\$PATH\""
    else
      say "$INSTALL_DIR is not on your PATH. Add it to your PATH, then restart your shell."
    fi
    ;;
esac
