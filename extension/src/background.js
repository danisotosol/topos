/*
This module listens for web requests to detect media streams
and communicates with the native host for casting.
*/

const api = globalThis.browser ?? globalThis.chrome;

const streams = {};
const devices = [];
let nativePort = null;

// Connects to the native host for communication with the extension
function connectNativeHost() {
  nativePort = api.runtime.connectNative("com.topos.cast");

  nativePort.onMessage.addListener((message) => {
    if (message.type === "DEVICES_FOUND") {
      devices.splice(0, devices.length, ...message.devices);
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
    const isStream =
      url.includes(".m3u8") || url.includes(".mpd") || url.includes("playlist");

    if (!isStream) return;

    if (!streams[tabId].includes(url)) {
      streams[tabId].push(url);
      console.log("[Topos] Stream detected:", url);
    }
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other"] },
  [],
);

// Deletes the stream from the object when the tab is loading
api.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "loading") {
    delete streams[tabId];
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
    nativePort.postMessage({
      type: "CAST_STREAM",
      url: message.url,
      device_id: message.device_id,
      title: message.title,
    });
  }

  if (message.type === "STREAM_DETECTED") {
    if (!sender.tab) return;

    const tabId = sender.tab?.id;

    if (!streams[tabId]) {
      streams[tabId] = [];
    }

    if (!streams[tabId].includes(message.url)) {
      streams[tabId].push(message.url);
      console.log("[Topos] Stream via injected:", message.url);
    }
  }
});
