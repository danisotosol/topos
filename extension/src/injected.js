(function () {
  if (window.__toposInjected) return;
  window.__toposInjected = true;

  function notifyIfStream(url, method) {
    if (
      url.includes(".m3u8") ||
      url.includes(".mpd") ||
      url.includes("playlist")
    ) {
      window.postMessage(
        { source: "TOPOS_INJECTED", type: "STREAM_DETECTED", method, url },
        "*",
      );
    }
  }

  // monkey patch
  if (window.fetch) {
    const originalFetch = window.fetch;
    window.fetch = function (input, options) {
      const url = typeof input === "string" ? input : input.url;
      notifyIfStream(url, "fetch");
      return originalFetch.apply(this, arguments);
    };
  }

  const originalXHROpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    notifyIfStream(String(url), "xhr");
    return originalXHROpen.apply(this, [method, url, ...rest]);
  };

  // observe changes to video source attributes because they may change dynamically
  const observer = new MutationObserver(function (mutations) {
    mutations.forEach(function (mutation) {
      if (mutation.attributeName === "src") {
        const el = mutation.target;
        if (el.tagName === "VIDEO" || el.tagName === "SOURCE") {
          const url = el.getAttribute("src");
          if (url) notifyIfStream(url, "video-src");
        }
      }
    });
  });

  const target = document.body || document.documentElement;
  observer.observe(target, { subtree: true, attributes: true });
})();
