<div align="center">

# Smart Download Manager (SDM)

<img alt="Written in Rust" src="https://img.shields.io/badge/Written_in-Rust-702DA0?style=for-the-badge&logo=rust&logoColor=white">
<img alt="Latest release" src="https://img.shields.io/github/v/release/rohityadav-sas/sdm?style=for-the-badge&color=702DA0&logo=rust&logoColor=white">
<img alt="GPL-3.0 licensed" src="https://img.shields.io/badge/license-GPL--3.0-1F9D55?style=for-the-badge">

**A blazing-fast, lightweight multi-connection download manager with browser integration.**

Up to 32 parallel connections · Dynamic segment splitting · Automatic resume · Bandwidth limiter · Queue management

</div>

---

## Features

- ⚡ **High Performance**: Saturate high-speed connections with up to 32 parallel HTTP range streams.
- 🔄 **Smart Splitting**: Dynamic segment stealing (steals from slower connections to eliminate stragglers).
- 💾 **Reliable Resuming**: Safe interruption and resumption; state saved automatically.
- 🧩 **Browser Integration**: Native messaging extension for Chrome, Edge, Brave, and Firefox with cookie & referer forwarding.
- 🎨 **Adaptive UI**: Modern interface with system dark/light theme switching and low resource footprint.
- 🚀 **Background Mode**: Downloads continue running silently when the window is closed.
- 🔗 **Direct URL Launch**: Pass download URLs directly from terminal or launchers (`sdm https://...`).

---

## Installation

### Quick install (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/rohityadav-sas/sdm/main/install.sh | sh
```

### Quick install (Windows PowerShell)

```powershell
irm https://raw.githubusercontent.com/rohityadav-sas/sdm/main/install.ps1 | iex
```

| OS | Install Path |
| --- | --- |
| **Linux / macOS** | `~/.local/bin/sdm` |
| **Windows** | `%LOCALAPPDATA%\Programs\sdm\sdm.exe` |

---

## Build from Source

### Prerequisites

- **Rust toolchain** (1.80+ recommended): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Linux dependencies**:
  - Fedora/RHEL: `sudo dnf install gcc fontconfig-devel`
  - Ubuntu/Debian: `sudo apt install build-essential libfontconfig1-dev`
  - Arch Linux: `sudo pacman -S base-devel fontconfig`

### Build & Local Install

```sh
git clone https://github.com/rohityadav-sas/sdm.git
cd sdm
cargo build --release
packaging/install.sh
```

To remove: `packaging/uninstall.sh`. To build a Fedora RPM: `packaging/make-rpm.sh`.

---

## Keyboard Shortcuts & Navigation

| Shortcut | Action |
| --- | --- |
| <kbd>Ctrl</kbd> + <kbd>V</kbd> | Paste URL(s) from clipboard and start downloading immediately |
| <kbd>Ctrl</kbd> + <kbd>N</kbd> | Open the New Download dialog |
| <kbd>Ctrl</kbd> + <kbd>,</kbd> | Open Preferences |
| <kbd>Ctrl</kbd> + <kbd>Q</kbd> | Pause all and quit |
| **Double-click** | Open finished download |

---

## Browser Extension Setup

1. Launch SDM, navigate to **Browser Extension** in the sidebar, and click **Set Up**. This registers `sdm-host` with your browser.
2. **Chrome / Edge / Brave**:
   - Open `chrome://extensions`
   - Enable **Developer mode** (top-right toggle)
   - Click **Load unpacked** and select the `chromium` folder from the opened directory.
3. **Firefox**:
   - Open `about:debugging` → **This Firefox**
   - Click **Load Temporary Add-on** and select `firefox/manifest.json`.

---

## Direct Download & Benchmarking

Send download URLs to SDM directly from your terminal or scripts:

```sh
# Start download (opens SDM or passes to running background instance)
sdm "https://example.com/largefile.iso"

# Run speed benchmark against aria2 and curl
scripts/bench.sh "https://example.com/largefile.iso" 3
```

---

## Architecture

- **`crates/engine`**: Core headless HTTP/1.1 multi-range download engine.
- **`crates/app`**: Slint-based graphical user interface and desktop integration.
- **`crates/ipc`**: Local communication channel connecting the GUI app and browser host.
- **`crates/host`**: Native messaging helper launched by browsers.
- **`crates/cli`**: Internal test & benchmarking harness.
- **`extension/`**: Browser extension for Chromium & Firefox.

For details, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
