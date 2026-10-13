# Prototype preview probe (P-0)

What a page in the Preview panel's iframe can reach — `docs/31-prototype-role.md`, P-0.
The pages run their checks and post them to the app's side (`parent.js`), which writes
each to `report.jsonl` here. Run it again after upgrading `tauri` or `wry`, and on Windows.

```bash
bun scripts/build-prototype-libs.ts scripts/prototype-probe/lib
```

```bash
CARGO_HOME=$HOME/.cargo RUSTUP_HOME=$HOME/.rustup HOME=/tmp/kibo-probe-home KIBO_PROTOTYPE_PROBE=$PWD/scripts/prototype-probe bun run tauri dev --config '{"identifier":"com.kibo.agent.probe"}'
```

`HOME` and the identifier keep it apart from an installed Kibo: the app's data lives in
`~/.kibo` whatever the identifier, and a second launch under the same one is handed to
the running app by single-instance. Six seconds after the window opens, two red-framed
iframes appear in its corner: `index.html` (React and JSX through Babel, served with the
preview's CSP) and `nocsp.html` (the same checks without it). After 15 s `report.jsonl`
ends with the parent's `alive` — the window was not navigated away. Close the window when
it does.

What to look for: no `__TAURI_INTERNALS__` in either frame, every IPC attempt refused,
`parent.*` and `top.location` refused, and with CSP no request leaving
(`violations` lists them).
