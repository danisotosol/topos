#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NATIVE_DIR="$SCRIPT_DIR/native-host"
BINARY_DEST="/usr/local/bin/topos-host"

echo "==> Building native host..."
cd "$NATIVE_DIR"
cargo build --release
cd "$SCRIPT_DIR"

echo "==> Installing binary to $BINARY_DEST..."
sudo cp "$NATIVE_DIR/target/release/topos-host" "$BINARY_DEST"
sudo chmod +x "$BINARY_DEST"

echo "==> Installing native host manifests..."

# Firefox
FIREFOX_DIR="$HOME/.mozilla/native-messaging-hosts"
mkdir -p "$FIREFOX_DIR"
cp "$NATIVE_DIR/com.topos.cast.firefox.json" "$FIREFOX_DIR/com.topos.cast.json"
echo "    Firefox: $FIREFOX_DIR/com.topos.cast.json"

# Chrome
CHROME_DIR="$HOME/.config/google-chrome/NativeMessagingHosts"
if [ -d "$HOME/.config/google-chrome" ]; then
  mkdir -p "$CHROME_DIR"
  cp "$NATIVE_DIR/com.topos.cast.chrome.json" "$CHROME_DIR/com.topos.cast.json"
  echo "    Chrome: $CHROME_DIR/com.topos.cast.json"
  echo "    NOTE: edit $CHROME_DIR/com.topos.cast.json and replace REPLACE_WITH_EXTENSION_HASH"
fi

# Chromium
CHROMIUM_DIR="$HOME/.config/chromium/NativeMessagingHosts"
if [ -d "$HOME/.config/chromium" ]; then
  mkdir -p "$CHROMIUM_DIR"
  cp "$NATIVE_DIR/com.topos.cast.chrome.json" "$CHROMIUM_DIR/com.topos.cast.json"
  echo "    Chromium: $CHROMIUM_DIR/com.topos.cast.json"
  echo "    NOTE: edit $CHROMIUM_DIR/com.topos.cast.json and replace REPLACE_WITH_EXTENSION_HASH"
fi

echo ""
echo "==> Firewall..."
if command -v ufw &>/dev/null && sudo ufw status | grep -q "Status: active"; then
  sudo ufw allow from 192.168.1.0/24 to any port 7070 comment "topos proxy"
  echo "    UFW rule added: LAN -> port 7070"
  echo "    NOTE: if your LAN subnet is not 192.168.1.0/24, run:"
  echo "      sudo ufw allow from <your-subnet>/24 to any port 7070"
elif command -v iptables &>/dev/null; then
  INPUT_POLICY=$(sudo iptables -L INPUT --line-numbers -n | head -1 | grep -o "policy [A-Z]*" | awk '{print $2}')
  if [ "$INPUT_POLICY" = "DROP" ] || ! sudo iptables -C INPUT -p tcp --dport 7070 -j ACCEPT 2>/dev/null; then
    sudo iptables -I INPUT -p tcp --dport 7070 -j ACCEPT
    echo "    iptables rule added: port 7070 ACCEPT"
    # Persist across reboots
    if [ -d /etc/iptables ]; then
      sudo sh -c "iptables-save > /etc/iptables/iptables.rules"
      sudo systemctl enable --now iptables 2>/dev/null || true
      echo "    Saved to /etc/iptables/iptables.rules (persistent)"
    else
      echo "    WARNING: /etc/iptables not found — rule will be lost on reboot."
      echo "    To persist: sudo iptables-save > /etc/iptables/iptables.rules"
    fi
  else
    echo "    iptables rule already present."
  fi
else
  echo "    No UFW or iptables found — open port 7070 manually if casting fails."
fi

echo ""
echo "Done. Load the extension in Firefox:"
echo "  about:debugging → This Firefox → Load Temporary Add-on → extension/manifest.json"
