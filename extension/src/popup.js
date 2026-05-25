const api = globalThis.browser ?? globalThis.chrome;

// Streams detected in the current tab
let currentStreams = [];
let currentTabUrl = "";

async function init() {
  // grab the first element of tab
  const [tab] = await api.tabs.query({ active: true, currentWindow: true });
  currentTabId = tab.id;
  currentTabUrl = tab.url ?? "";

  // Ask background for streams detected in this tab
  const { streams } = await api.runtime.sendMessage({
    type: "GET_STREAMS",
    tabId: tab.id,
  });
  renderStreams(streams);

  // Ask background for cached devices
  const { devices } = await api.runtime.sendMessage({ type: "GET_DEVICES" });
  renderDevices(devices);

  // Restore cast state if already casting
  const { deviceId } = await api.runtime.sendMessage({ type: "GET_CAST_STATE" });
  if (deviceId) {
    const btn = document.querySelector(`.device[data-device-id="${CSS.escape(deviceId)}"]`);
    if (btn) {
      btn.classList.add("casting");
      const badge = document.createElement("span");
      badge.className = "live-badge";
      badge.textContent = "live";
      btn.appendChild(badge);
    }
  }
}

function renderStreams(urls) {
  currentStreams = urls;

  const list = document.getElementById("stream-list");
  const count = document.getElementById("stream-count");
  count.textContent = urls.length;

  if (urls.length === 0) {
    list.innerHTML =
      '<div class="empty"><strong>No streams detected</strong>Play a video — streams appear automatically</div>';
    return;
  }

  list.innerHTML = "";
  urls.forEach((url) => {
    const filename = url.split("/").pop().split("?")[0] || url;
    const origin = (() => {
      try {
        return new URL(url).hostname;
      } catch {
        return url;
      }
    })();
    const isDash = url.includes(".mpd");
    const type = isDash ? "dash" : "hls";
    const label = isDash ? "DASH" : "HLS";

    const row = document.createElement("div");
    row.className = "stream";
    row.innerHTML = `
      <div class="stream-info">
        <div class="stream-title">${filename}</div>
        <div class="stream-origin">${origin}</div>
      </div>
      <div class="stream-meta">
        <span class="chip chip-${type}">${label}</span>
      </div>`;
    list.appendChild(row);
  });
}

function renderDevices(devices) {
  const list = document.getElementById("device-list");
  const count = document.getElementById("device-count");
  count.textContent = devices.length;

  if (devices.length === 0) {
    list.innerHTML =
      '<div class="empty"><strong>No devices found</strong>Click rescan to search the network</div>';
    return;
  }

  list.innerHTML = "";
  devices.forEach((device) => {
    const btn = document.createElement("button");
    btn.className = "device";
    btn.dataset.deviceId = device.id;
    btn.innerHTML = `
      <div class="device-left">
        <div class="device-icon">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 16a5 5 0 0 1 5 5"/><path d="M3 12a9 9 0 0 1 9 9"/>
            <path d="M3 8a13 13 0 0 1 13 13"/><rect x="3" y="4" width="18" height="14" rx="2"/>
          </svg>
        </div>
        <div>
          <div class="device-name">${device.name}</div>
          <div class="device-kind">${device.ip}</div>
        </div>
      </div>`;

    btn.addEventListener("click", () => {
      const isCasting = btn.classList.contains("casting");

      document.querySelectorAll(".device").forEach((d) => {
        d.classList.remove("casting");
        const badge = d.querySelector(".live-badge");
        if (badge) badge.remove();
      });

      if (!isCasting) {
        if (currentStreams.length === 0) return;

        btn.classList.add("casting");
        const badge = document.createElement("span");
        badge.className = "live-badge";
        badge.textContent = "live";
        btn.appendChild(badge);

        const url = currentStreams[0];
        api.runtime.sendMessage({
          type: "CAST_STREAM",
          url,
          device_id: device.id,
          device_ip: device.ip,
          device_port: device.port,
          title: device.name,
          referer: currentTabUrl,
        });
      } else {
        api.runtime.sendMessage({ type: "STOP_CAST" });
      }
    });

    list.appendChild(btn);
  });
}

// Live stream updates: background broadcasts when a new stream is detected
let currentTabId = null;
api.runtime.onMessage.addListener((message) => {
  if (message.type === "STREAMS_UPDATED" && message.tabId === currentTabId) {
    renderStreams(message.streams);
  }
});

// Rescan button: ask background to trigger mDNS scan, refresh device list after 2s
document.getElementById("btn-rescan").addEventListener("click", () => {
  const btn = document.getElementById("btn-rescan");
  btn.classList.add("scanning");

  api.runtime.sendMessage({ type: "SCAN_DEVICES" });

  setTimeout(async () => {
    btn.classList.remove("scanning");
    const { devices } = await api.runtime.sendMessage({ type: "GET_DEVICES" });
    renderDevices(devices);
  }, 2000);
});

init();
