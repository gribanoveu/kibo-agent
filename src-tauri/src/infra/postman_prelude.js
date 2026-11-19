// What uvm's firmware gives the bootcode, without a Worker: timers the host waits
// out, and the bridge's two ends as plain calls. The bootcode deletes every global
// it does not know, so what the host needs later is kept here and returned.
(function (g, Flatted, emit) {
  let now = 0, seq = 0, dispatch = null;
  const timers = new Map();
  const add = (fn, ms, args, every) => {
    const id = ++seq;
    const delay = Math.max(0, Number(ms) || 0);
    timers.set(id, { fn, at: now + delay, args, every: every ? Math.max(1, delay) : 0 });
    return id;
  };
  const next = () => {
    let best = null;
    for (const entry of timers) if (!best || entry[1].at < best[1].at) best = entry;
    return best;
  };
  g.setTimeout = (fn, ms, ...a) => add(fn, ms, a, false);
  g.setInterval = (fn, ms, ...a) => add(fn, ms, a, true);
  g.clearTimeout = g.clearInterval = (id) => { timers.delete(id); };
  // The browser build of `buffer` needs these to load; scripts rarely touch them.
  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  g.btoa = (s) => {
    s = String(s); let o = "";
    for (let i = 0; i < s.length; i += 3) {
      const n = (s.charCodeAt(i) << 16) | ((s.charCodeAt(i + 1) || 0) << 8) | (s.charCodeAt(i + 2) || 0);
      o += B64[n >> 18 & 63] + B64[n >> 12 & 63] + (i + 1 < s.length ? B64[n >> 6 & 63] : "=") + (i + 2 < s.length ? B64[n & 63] : "=");
    }
    return o;
  };
  g.atob = (s) => {
    s = String(s).replace(/[=\s]/g, ""); let o = "", bits = 0, acc = 0;
    for (const c of s) { acc = (acc << 6) | B64.indexOf(c); bits += 6; if (bits >= 8) { bits -= 8; o += String.fromCharCode(acc >> bits & 255); } }
    return o;
  };
  g.Blob = class Blob { constructor(parts = [], opts = {}) { this.parts = parts; this.type = opts.type || ""; } };
  g.File = class File extends g.Blob { constructor(parts, name, opts) { super(parts, opts); this.name = name; } };
  g.__uvm_emit = (s) => emit(JSON.stringify(Flatted.parse(s)));
  return {
    // How long until the next timer, in milliseconds; -1 when none is left.
    due() {
      const best = next();
      return best ? Math.max(0, best[1].at - now) : -1;
    },
    // The next timer fired; the host has waited for it.
    fire() {
      const best = next();
      if (!best) return;
      const [id, t] = best;
      now = t.at;
      if (t.every) t.at += t.every; else timers.delete(id);
      t.fn(...t.args);
    },
    bind(d) { dispatch = d; },
    send(name, json) { dispatch(Flatted.stringify([name, ...JSON.parse(json)])); },
  };
})
