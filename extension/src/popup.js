const api = globalThis.browser ?? globalThis.chrome;

// Streams detected in the current tab
let currentStreams = [];
let currentTabUrl = "";
let isPaused = false;
let knownTime = 0;       // last confirmed position from Chromecast (seconds)
let knownAt = 0;         // Date.now() when knownTime was set
let totalDuration = 0;   // total media duration (seconds), 0 = unknown
let seekInterval = null;
let isSeeking = false;   // true while user is dragging the slider
let castTimeout = null;  // reverts the optimistic casting UI if the host never confirms

// Updates the footer status line + dot color. kind: "ok" | "warn" | "error".
function setStatus(text, kind) {
  const dot = document.getElementById("status-dot");
  const txt = document.getElementById("status-text");
  if (txt) txt.textContent = text;
  if (dot) dot.className = "status-dot" + (kind === "error" ? " error" : kind === "warn" ? " warn" : "");
}

function clearCastTimeout() {
  if (castTimeout) clearTimeout(castTimeout);
  castTimeout = null;
}

// Drops the casting highlight/badge from every device and hides the controls.
function revertCastingUI() {
  document.querySelectorAll(".device").forEach((d) => {
    d.classList.remove("casting");
    const badge = d.querySelector(".live-badge");
    if (badge) badge.remove();
  });
  hideControls();
}

// Escapes untrusted strings (filenames, mDNS device names) before they go into
// innerHTML — prevents HTML/UI injection from network-controlled values.
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

function formatTime(secs) {
  const s = Math.max(0, Math.floor(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = s % 60;
  const mm = String(m).padStart(2, "0");
  const zz = String(ss).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${zz}` : `${mm}:${zz}`;
}

function setSliderProgress(slider, value, max) {
  // Paint the filled portion of the track using a gradient
  const pct = max > 0 ? (value / max) * 100 : 0;
  slider.style.background = `linear-gradient(to right, var(--accent) ${pct}%, var(--border-strong) ${pct}%)`;
}

function syncSlider(currentTime) {
  const slider = document.getElementById("seek-bar");
  const timeEl = document.getElementById("ctrl-time");
  slider.value = currentTime;
  setSliderProgress(slider, currentTime, totalDuration);
  const durStr = totalDuration > 0 ? ` / ${formatTime(totalDuration)}` : "";
  timeEl.textContent = formatTime(currentTime) + durStr;
}

function startTimeTracking() {
  if (seekInterval) clearInterval(seekInterval);
  seekInterval = setInterval(() => {
    if (!isSeeking) {
      // Estimate current position locally — no reconnection needed
      const estimated = isPaused ? knownTime : knownTime + (Date.now() - knownAt) / 1000;
      syncSlider(Math.min(estimated, totalDuration || Infinity));
    }
  }, 1000);
}

function stopTimeTracking() {
  if (seekInterval) clearInterval(seekInterval);
  seekInterval = null;
}

function showControls(deviceName) {
  document.getElementById("controls-label").textContent = `Now casting · ${deviceName}`;
  document.getElementById("controls").style.display = "";
  startTimeTracking();
}

function hideControls() {
  document.getElementById("controls").style.display = "none";
  stopTimeTracking();
  isPaused = false;
  knownTime = 0;
  totalDuration = 0;
}

function updateCastState(playerState, currentTime, duration) {
  isPaused = playerState === "PAUSED";
  const btn = document.getElementById("btn-pause-play");
  btn.textContent = isPaused ? "▶ Play" : "⏸ Pause";
  btn.className = isPaused ? "btn-ctrl" : "btn-ctrl primary";

  knownTime = currentTime;
  knownAt = Date.now();

  if (duration != null && duration > 0) {
    totalDuration = duration;
    document.getElementById("seek-bar").max = duration;
  }

  syncSlider(currentTime);
}

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
      const name = btn.querySelector(".device-name")?.textContent ?? "device";
      showControls(name);
      // Ask native host for current position (reconnects once to query state)
      api.runtime.sendMessage({ type: "QUERY_CAST_STATE" });
    }
  }

  // Playback controls
  document.getElementById("btn-pause-play").addEventListener("click", () => {
    api.runtime.sendMessage({ type: isPaused ? "PLAY_CAST" : "PAUSE_CAST" });
  });

  document.getElementById("btn-skip-back").addEventListener("click", () => {
    const pos = isPaused ? knownTime : knownTime + (Date.now() - knownAt) / 1000;
    api.runtime.sendMessage({ type: "SEEK_CAST", position: Math.max(0, pos - 10) });
  });

  document.getElementById("btn-skip-fwd").addEventListener("click", () => {
    const pos = isPaused ? knownTime : knownTime + (Date.now() - knownAt) / 1000;
    api.runtime.sendMessage({ type: "SEEK_CAST", position: pos + 10 });
  });

  const slider = document.getElementById("seek-bar");

  // While dragging: update display only (no network call)
  slider.addEventListener("input", () => {
    isSeeking = true;
    const val = parseFloat(slider.value);
    syncSlider(val);
  });

  // On release: send the seek command
  slider.addEventListener("change", () => {
    isSeeking = false;
    const val = parseFloat(slider.value);
    api.runtime.sendMessage({ type: "SEEK_CAST", position: val });
  });
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
        <div class="stream-title">${escapeHtml(filename)}</div>
        <div class="stream-origin">${escapeHtml(origin)}</div>
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
          <div class="device-name">${escapeHtml(device.name)}</div>
          <div class="device-kind">${escapeHtml(device.ip)}</div>
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
        showControls(device.name);
        setStatus(`Connecting to ${device.name}…`, "warn");
        // Revert the optimistic UI if the host never confirms (CAST_STARTED/CAST_STATE)
        clearCastTimeout();
        castTimeout = setTimeout(() => {
          revertCastingUI();
          setStatus("Cast timed out — no response from host", "error");
        }, 12000);

        const url = currentStreams[0];
        api.runtime.sendMessage({
          type: "CAST_STREAM",
          url,
          device_id: device.id,
          device_ip: device.ip,
          device_port: device.port,
          title: device.name,
          referer: currentTabUrl,
          tab_id: currentTabId,
        });
      } else {
        clearCastTimeout();
        hideControls();
        setStatus("native host connected", "ok");
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
  if (message.type === "CAST_STATE") {
    clearCastTimeout();
    updateCastState(message.player_state, message.current_time, message.duration);
  }
  if (message.type === "DEVICES_UPDATED") {
    renderDevices(message.devices);
  }
  if (message.type === "CAST_STARTED") {
    // Real confirmation from the host — cancel the timeout and mark casting live
    clearCastTimeout();
    setStatus("Casting", "ok");
  }
  if (message.type === "CAST_ERROR") {
    // Cast/host failure — drop the optimistic UI and show the reason to the user
    clearCastTimeout();
    revertCastingUI();
    setStatus(message.error || "Cast failed", "error");
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
