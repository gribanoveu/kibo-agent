// Evaluated in the main window by `prototype_probe.rs`: the app's side — two
// sandboxed iframes, as the Preview panel would draw them, and every message
// they send written to `report.jsonl` through the protocol.
(() => {
  const base = "artifact://localhost";
  const report = (data) =>
    fetch(`${base}/__probe`, { method: "POST", body: JSON.stringify(data) }).catch((e) => console.error("probe", e));
  const frames = ["index.html", "nocsp.html"].map((page, i) => {
    const frame = document.createElement("iframe");
    frame.sandbox = "allow-scripts allow-modals allow-forms";
    frame.src = `${base}/${page}`;
    frame.style.cssText = `position:fixed;right:8px;bottom:${8 + i * 170}px;width:360px;height:160px;z-index:99999;background:#fff;border:2px solid red`;
    frame.onload = () => report({ from: "parent", event: "load", page });
    document.body.appendChild(frame);
    return frame;
  });
  addEventListener("message", (e) => {
    if (!frames.some((f) => f.contentWindow === e.source)) return;
    report({ from: "iframe", origin: e.origin, data: e.data });
  });
  report({ from: "parent", event: "start", href: location.href });
  setTimeout(() => report({ from: "parent", event: "alive", href: location.href }), 15000);
})();
