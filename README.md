<p align="center">
  <img src="extension/icons/icon128.png" alt="Topos_icon"/>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.1.0--alpha-pink?style=flat-square"/>
  <img src="https://img.shields.io/badge/platform-Linux-lightgrey?style=flat-square"/>
  <img src="https://img.shields.io/badge/Firefox-109%2B-orange?style=flat-square"/>
  <img src="https://img.shields.io/badge/Chrome%2FChromium-supported-green?style=flat-square"/>
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square"/>
</p>

---

**Topos** is a Linux browser extension that streams video from any web page to your Chromecast or Google TV — no accounts, no servers, no telemetry.

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

- Linux
- Firefox 109+ or Chrome/Chromium
- Rust toolchain (`rustup`)
- A Chromecast, Google TV, or Android TV on the same local network

---

## Install

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

---

## Usage

1. Open any page with a video
2. Click the Topos icon in the toolbar
3. Click **Rescan** to find devices on your network
4. A stream is detected automatically — click a device to cast
5. Use the **pause / play / seek** controls in the popup while casting

---

## Firewall note

The Chromecast fetches the stream from your machine on port `7070`. If you run UFW or another firewall, you must allow it:

```bash
sudo ufw allow from 192.168.1.0/24 to any port 7070
```

Replace `192.168.1.0/24` with your actual LAN subnet if different.

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
