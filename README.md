<p align="center">
  <img src="extension/icons/icon128.png" alt="Topos_icon"/>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.1.0--alpha-pink?style=flat-square"/>
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20Windows-lightgrey?style=flat-square"/>
  <img src="https://img.shields.io/badge/Firefox-109%2B-orange?style=flat-square"/>
  <img src="https://img.shields.io/badge/Chrome%2FChromium-supported-green?style=flat-square"/>
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square"/>
</p>

---

**Topos** is a Linux and Windows browser extension that streams video from any web page to your Chromecast or Google TV — no accounts, no servers, no telemetry.

Detects HLS (`.m3u8`) and DASH (`.mpd`) streams by intercepting network requests and `fetch`/`XHR` calls. Sends them directly to your device over the local network using the Google Cast protocol.

---

## Features

- **Auto-detect streams** — intercepts `.m3u8` / `.mpd` requests and `fetch`/`XHR` calls from any page
- **Local proxy** — re-routes the stream through your machine so auth tokens stay valid on the Chromecast
- **Subtitles** — detects external VTT subtitle files and sends them as active text tracks
- **Playback controls** — pause, play, seek from the popup without leaving the browser
- **Cast state persists** — closing and reopening the popup keeps the casting indicator and controls
- **mDNS discovery** — finds all Chromecast / Google TV devices on the local network automatically

---

## How it works

```
Browser (JS extension)
  │  intercepts .m3u8 / .mpd requests + fetch/XHR patches
  │  Native Messaging Protocol (stdin/stdout)
  ▼
topos-host (Rust binary)
  │  HTTP proxy on port 7070 — rewrites HLS manifests, forwards auth headers
  │  mDNS — discovers Chromecasts on local network
  │  Google Cast protocol (TLS + protobuf, port 8009)
  ▼
Chromecast / Google TV
```

The browser extension is minimal JavaScript glue. All real logic — proxy, Cast protocol, mDNS discovery — lives in the native host written in Rust.

---

## Requirements

- Linux or Windows 10/11
- Firefox 109+ or Chrome/Chromium
- Rust toolchain (`rustup`)
- A Chromecast, Google TV, or Android TV on the same local network

---

## Install

### Linux

```bash
git clone https://github.com/danisotosol/topos
cd topos
bash install.sh
```

`install.sh` compiles the binary, installs it, registers the native host manifests for Firefox and Chrome/Chromium, and opens port 7070 through UFW if the firewall is active.

**Manual steps if you prefer:**

```bash
# Build and install the native host
cd native-host
cargo build --release
sudo cp target/release/topos-host /usr/local/bin/
cd ..

# Register the native host for Firefox
mkdir -p ~/.mozilla/native-messaging-hosts
cp native-host/com.topos.cast.firefox.json ~/.mozilla/native-messaging-hosts/com.topos.cast.json

# Open the proxy port if UFW is active
sudo ufw allow from 192.168.1.0/24 to any port 7070
```

Then load the extension:

**Firefox:** `about:debugging` → This Firefox → Load Temporary Add-on → select `extension/manifest.json`

**Chrome/Chromium:** `chrome://extensions` → Developer mode → Load unpacked → select `extension/`

### Windows

```powershell
git clone https://github.com/danisotosol/topos
cd topos
powershell -ExecutionPolicy Bypass -File native-host\install.ps1
```

`install.ps1` compiles the binary, installs it to `%LOCALAPPDATA%\Topos\topos-host.exe`, registers the native messaging host for Firefox and Chrome, and adds a Windows Firewall rule for port 7070 — the only step that needs Administrator rights.

Unlike Linux, Windows browsers locate the native host manifest through the registry. The script creates these keys under the current user, each pointing to the generated manifest file:

```
HKEY_CURRENT_USER\Software\Mozilla\NativeMessagingHosts\com.topos.cast
HKEY_CURRENT_USER\Software\Google\Chrome\NativeMessagingHosts\com.topos.cast
```

Loading the extension works exactly as on Linux: `about:debugging` in Firefox, `chrome://extensions` in Chrome.

---

## Releases

Each tagged release ships two platform artifacts:

- `topos-linux-<tag>.tar.gz` — extension + native host + `install.sh`
- `topos-windows-<tag>.zip` — extension + native host + `install.ps1`

Both are built automatically by GitHub Actions (`.github/workflows/release.yml`) whenever a tag is pushed. Download the archive for your platform, extract it, and run the installer inside.

---

## Usage

1. Open any page with a video
2. Click the Topos icon in the toolbar
3. Click **Rescan** to find devices on your network
4. A stream is detected automatically — click a device to cast
5. Use the **pause / play / seek** controls in the popup while casting

---

## Firewall note

The Chromecast fetches the stream from your machine on port `7070`. `install.sh`/`install.ps1` try to open it automatically; if that's skipped, allow it manually.

**Linux** (UFW or another firewall):

```bash
sudo ufw allow from 192.168.1.0/24 to any port 7070
```

Replace `192.168.1.0/24` with your actual LAN subnet if different.

**Windows** (elevated PowerShell):

```powershell
New-NetFirewallRule -DisplayName "Topos proxy" -Direction Inbound -Protocol TCP -LocalPort 7070 -Action Allow -Profile Private,Domain -RemoteAddress LocalSubnet
```

---

## Supported devices

Any device that speaks Google Cast:

- Chromecast (all generations)
- Google TV
- Android TV (most models)
- Sony / TCL / Hisense TVs with Google TV built in

---

## License

MIT
