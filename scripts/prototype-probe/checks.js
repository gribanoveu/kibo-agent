// What a page in the preview iframe can reach (docs/31-prototype-role.md, P-0).
// Each check records what happened; nothing here decides pass or fail — the
// report is read by a person.

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const violations = [];
addEventListener("securitypolicyviolation", (e) => violations.push(`${e.effectiveDirective} ${e.blockedURI}`));

async function attempt(fn) {
  try {
    return { ok: true, value: await fn() };
  } catch (e) {
    return { ok: false, error: `${e?.name ?? "Error"}: ${e?.message ?? e}` };
  }
}

// A raw IPC request, as `@tauri-apps/api` would send it, without the invoke key.
const ipcFetch = (base) =>
  attempt(async () => {
    const res = await fetch(`${base}/viewer_text_scale_get`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "Tauri-Callback": "1", "Tauri-Error": "2" },
      body: "{}",
    });
    return `${res.status} ${await res.text()}`;
  });

// macOS's other way in: the script message handler, answered through a callback.
const webkitIpc = () =>
  attempt(async () => {
    const handler = window.webkit?.messageHandlers?.ipc;
    if (!handler) return "no handler";
    let answered = null;
    window._9001 = (v) => (answered = { callback: v });
    window._9002 = (v) => (answered = { error: v });
    handler.postMessage(JSON.stringify({ cmd: "viewer_text_scale_get", callback: 9001, error: 9002, payload: {} }));
    await wait(1500);
    return answered ?? "no answer";
  });

export async function run(label) {
  const results = {
    label,
    origin: self.origin,
    href: location.href,
    globals: {
      __TAURI_INTERNALS__: typeof window.__TAURI_INTERNALS__,
      __TAURI__: typeof window.__TAURI__,
      ipc: typeof window.ipc,
      webkitIpcHandler: typeof window.webkit?.messageHandlers?.ipc,
    },
    invokeViaInternals: window.__TAURI_INTERNALS__
      ? await attempt(() => window.__TAURI_INTERNALS__.invoke("viewer_text_scale_get"))
      : "no internals",
    fetchIpcScheme: await ipcFetch("ipc://localhost"),
    fetchIpcHttp: await ipcFetch("http://ipc.localhost"),
    webkitPostMessage: await webkitIpc(),
    parentDocument: await attempt(() => typeof parent.document.body),
    parentInternals: await attempt(() => typeof parent.__TAURI_INTERNALS__),
    fetchExternal: await attempt(async () => (await fetch("https://example.com/")).status),
    imageExternal: await new Promise((resolve) => {
      const img = new Image();
      img.onload = () => resolve("loaded");
      img.onerror = () => resolve("error");
      img.src = "https://example.com/favicon.ico";
      setTimeout(() => resolve("timeout"), 3000);
    }),
    windowOpen: await attempt(() => String(window.open("about:blank"))),
    localStorage: await attempt(() => localStorage.length),
    cookie: await attempt(() => document.cookie),
  };
  await wait(500);
  results.violations = violations;
  parent.postMessage({ kind: "results", results }, "*");

  // Last: if this works, the app's window is gone.
  const topNav = await attempt(() => {
    top.location.href = "about:blank#escaped-from-prototype";
    return "navigated";
  });
  parent.postMessage({ kind: "topNavigation", label, topNav }, "*");
}
