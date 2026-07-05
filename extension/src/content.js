const api = globalThis.browser ?? globalThis.chrome;

(function () {
  if (window.__toposContent) return;
  window.__toposContent = true;

  const script = document.createElement("script");
  script.src = api.runtime.getURL("src/injected.js");
  script.onload = () => script.remove();
  document.documentElement.appendChild(script);
})();

window.addEventListener("message", (event) => {
  // Only trust messages from this same window with a well-formed payload — any page,
  // iframe, or ad can postMessage, so validate before relaying to the background.
  if (event.source !== window) return;
  const data = event.data;
  if (!data || typeof data !== "object") return;
  if (data.source !== "TOPOS_INJECTED" || data.type !== "STREAM_DETECTED") return;
  try {
    const u = new URL(data.url);
    if (u.protocol !== "http:" && u.protocol !== "https:") return;
  } catch {
    return;
  }
  api.runtime.sendMessage({
    source: "TOPOS_INJECTED",
    type: "STREAM_DETECTED",
    url: data.url,
    method: data.method,
  });
});

// YouTube and similar sites add <video> dynamically after the page loads
function addCastButton(video) {
  if (video.__toposCastButton) return;
  video.__toposCastButton = true;

  const button = document.createElement("button");
  button.textContent = "Cast";
  button.style.cssText = "position:absolute;z-index:9999;top:10px;right:10px;";

  video.parentElement.style.position = "relative";
  video.parentElement.appendChild(button);
}

const observer = new MutationObserver((mutations) => {
  for (const mutation of mutations) {
    for (const node of mutation.addedNodes) {
      if (node.nodeName === "VIDEO") addCastButton(node);
    }
  }
});

observer.observe(document.documentElement, {
  childList: true,
  subtree: true,
});
