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

## How it works

```
Browser (JS extension)
  │  intercepts .m3u8 / .mpd requests
  │  Native Messaging Protocol (stdin/stdout)
  ▼
topos-host (Rust binary)
  │  mDNS — discovers Chromecasts on local network
  │  Google Cast protocol (TLS + protobuf, port 8009)
  ▼
Chromecast / Google TV
```

The browser extension is minimal JavaScript glue (~100 lines). All real logic lives in the native host binary written in Rust.

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

# Build and install the native host
cd native-host
cargo build --release
sudo cp target/release/topos-host /usr/local/bin/
cd ..

# Register the native host for Firefox
mkdir -p ~/.mozilla/native-messaging-hosts
cp native-host/com.topos.cast.firefox.json ~/.mozilla/native-messaging-hosts/com.topos.cast.json
```

Then load the extension:

**Firefox:** `about:debugging` → This Firefox → Load Temporary Add-on → select `extension/manifest.json`

**Chrome/Chromium:** `chrome://extensions` → Developer mode → Load unpacked → select `extension/`

---

## Usage

1. Open any page with a video (Twitch, a news site, any HLS/DASH stream)
2. Click the Topos icon in the toolbar
3. Click **Scan** to find devices on your network
4. Select a stream → select a device → **Cast**

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
