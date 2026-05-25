/*
This module listens for web requests to detect media streams
and communicates with the native host for casting.
*/

const api = globalThis.browser ?? globalThis.chrome;

const streams = {};
const devices = [];
let nativePort = null;
let castingDeviceId = null;

// Restore streams from session storage on service worker startup
api.storage.session.get(null).then((saved) => {
  for (const [key, value] of Object.entries(saved)) {
    if (key.startsWith("streams_")) {
      streams[parseInt(key.replace("streams_", ""))] = value;
    }
  }
});

// Connects to the native host for communication with the extension
function connectNativeHost() {
  nativePort = api.runtime.connectNative("com.topos.cast");

  nativePort.onMessage.addListener((message) => {
    if (message.type === "DEVICES_FOUND") {
      devices.splice(0, devices.length, ...message.devices);
    } else if (message.type === "CAST_STARTED") {
      console.log("[Topos] Cast started on:", message.device);
      console.log("[Topos] Proxy URL (open in Firefox to test):", message.proxy_url);
    } else if (message.type === "CAST_ERROR") {
      console.error("[Topos] Cast error:", message.error);
    }
  });

  nativePort.onDisconnect.addListener(() => {
    nativePort = null;
  });
}

// Listens for web requests to detect media streams
api.webRequest.onBeforeRequest.addListener(
  (details) => {
    const url = details.url;
    const tabId = details.tabId;

    if (tabId < 0) return;

    if (!streams[tabId]) {
      streams[tabId] = [];
    }

    // Checks if the URL contains a media stream extension
    const isStream = url.includes(".m3u8") || url.includes(".mpd");

    if (!isStream) return;

    if (!streams[tabId].includes(url)) {
      streams[tabId].push(url);
      api.storage.session.set({ [`streams_${tabId}`]: streams[tabId] });
      console.log("[Topos] Stream detected:", url);
      api.runtime.sendMessage({ type: "STREAMS_UPDATED", tabId, streams: streams[tabId] }).catch(() => {});
    }
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other"] },
  [],
);

// Deletes streams only on real navigation (URL change), not SPA internal route changes
api.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "loading" && changeInfo.url) {
    delete streams[tabId];
    api.storage.session.remove(`streams_${tabId}`);
  }
});

// Handles incoming messages from the extension's content script
api.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.type === "GET_STREAMS") {
    const tabId = message.tabId;
    // Returns the streams for the given tab, or an empty array if none are found
    sendResponse({ streams: streams[tabId] ?? [] });
  }

  if (message.type === "GET_DEVICES") {
    sendResponse({ devices: devices });
  }

  if (message.type === "SCAN_DEVICES") {
    if (!nativePort) connectNativeHost();
    nativePort.postMessage({ type: "SCAN_DEVICES" });
  }

  if (message.type === "CAST_STREAM") {
    if (!nativePort) connectNativeHost();
    castingDeviceId = message.device_id;
    // Extract cookies for the stream domain so the proxy can impersonate the browser session
    (async () => {
      let cookieHeader = "";
      try {
        const origin = new URL(message.url).origin;
        const jar = await api.cookies.getAll({ url: origin });
        cookieHeader = jar.map((c) => `${c.name}=${c.value}`).join("; ");
        console.log("[Topos] Cookies for proxy:", jar.length, "cookies,", cookieHeader.length, "bytes");
      } catch (e) {
        console.warn("[Topos] Could not read cookies:", e);
      }
      const referer = message.referer ?? "";
      console.log("[Topos] Referer for proxy:", referer || "(none)");
      nativePort.postMessage({
        type: "CAST_STREAM",
        url: message.url,
        device_id: message.device_id,
        device_ip: message.device_ip,
        device_port: message.device_port,
        title: message.title,
        cookies: cookieHeader,
        referer,
      });
    })();
    return; // fire-and-forget, no sendResponse needed
  }

  if (message.type === "STOP_CAST") {
    castingDeviceId = null;
    if (nativePort) nativePort.postMessage({ type: "STOP_CAST" });
  }

  if (message.type === "GET_CAST_STATE") {
    sendResponse({ deviceId: castingDeviceId });
  }

  if (message.type === "STREAM_DETECTED") {
    if (!sender.tab) return;

    const tabId = sender.tab?.id;

    if (!streams[tabId]) {
      streams[tabId] = [];
    }

    if (!streams[tabId].includes(message.url)) {
      streams[tabId].push(message.url);
      api.storage.session.set({ [`streams_${tabId}`]: streams[tabId] });
      console.log("[Topos] Stream via injected:", message.url);
      api.runtime.sendMessage({ type: "STREAMS_UPDATED", tabId, streams: streams[tabId] }).catch(() => {});
    }
  }
});
