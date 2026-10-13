// Evaluated in the main window by `prototype_probe.rs`: the app's side — a
// sandboxed iframe on the real `artifact://`, as the Preview panel draws it,
// and every message it sends handed back to Rust on the `prototype-probe` event.
(() => {
  const report = (data) =>
    window.__TAURI_INTERNALS__
      .invoke("plugin:event|emit", { event: "prototype-probe", payload: data })
      .catch((e) => console.error("probe", e));
  const frame = document.createElement("iframe");
  frame.sandbox = "allow-scripts allow-modals allow-forms";
  frame.src = "artifact://localhost/p0-probe/files/index.html";
  frame.style.cssText =
    "position:fixed;right:8px;bottom:8px;width:360px;height:160px;z-index:99999;background:#fff;border:2px solid red";
  let loads = 0;
  frame.onload = () => report({ from: "parent", event: "load", count: ++loads });
  // A frame navigation `frame-src` refuses is reported here, in the window.
  document.addEventListener("securitypolicyviolation", (e) =>
    report({ from: "parent", event: "violation", directive: e.effectiveDirective, blocked: e.blockedURI }),
  );
  document.body.appendChild(frame);
  addEventListener("message", (e) => {
    if (e.source !== frame.contentWindow) return;
    report({ from: "iframe", origin: e.origin, data: e.data });
  });
  report({ from: "parent", event: "start", href: location.href });
  setTimeout(() => report({ from: "parent", event: "alive", href: location.href }), 15000);
})();
