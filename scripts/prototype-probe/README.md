# Prototype preview probe (P-0)

What a page in the Preview panel's iframe can reach — `docs/31-prototype-role.md`, P-0.
The page is served by the app's own `artifact://`, with its real headers, from a chat
folder of its own (`p0-probe`); its checks go to the window (`parent.js`) and back to
Rust on a Tauri event, into `report.jsonl` here. Run it again after upgrading `tauri`
or `wry` — `prototype_protocol::tests::tauri_and_wry_are_the_versions_the_preview_probe_checked`
fails until you do — and on Windows.

Built, so the window's CSP applies (`tauri dev` serves the window without it, and the
frame's own navigation away is then not stopped):

```bash
bun run tauri build --debug --no-bundle --config '{"identifier":"com.kibo.agent.probe"}'
```

```bash
HOME=/tmp/kibo-probe-home KIBO_PROTOTYPE_PROBE=$PWD/scripts/prototype-probe ~/.cargo-target/debug/kibo-agent
```

`HOME` and the identifier keep it apart from an installed Kibo: the app's data lives in
`~/.kibo` whatever the identifier, and a second launch under the same one is handed to
the running app by single-instance. (The binary is wherever Cargo puts this project's
target.) Six seconds after the window opens a red-framed iframe appears in its corner;
after 15 s `report.jsonl` ends with the parent's `alive`. Close the window then.

What to look for: the page rendered; no `__TAURI_INTERNALS__` in the frame; every IPC
attempt refused; `parent.*` and `top.location` refused; `violations` listing each
request out; and a `violation` from the parent for `frame-src` with only one `load` —
the frame did not leave for `example.com`.
