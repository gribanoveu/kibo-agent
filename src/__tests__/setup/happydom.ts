// Registers happy-dom's globals (document, window, …) before any test runs,
// so `@testing-library/react` can render. Loaded via `bunfig.toml`'s
// `[test] preload` — Bun's own runner has no DOM of its own.
//
// Pure-function tests in this directory neither need nor notice it.
import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { afterEach } from "bun:test";

GlobalRegistrator.register();

// Unmount whatever the finished test rendered. Registered here rather than
// repeated in every file because forgetting it is silent until it isn't: a
// leaked render keeps its effects — and its global event listeners — alive
// for the rest of the file, so one dispatched `window` event reaches every
// instance the file ever created, not just the one under test.
//
// Imported dynamically, after `register()`: a static import is hoisted above
// it, and `@testing-library/react` wants the DOM globals at module-load time.
const { cleanup } = await import("@testing-library/react");

// The real modules first, so they fix the export set. Bun keeps the exports
// of whichever version of a module was loaded first and later `mock.module`
// calls only replace their values: were a test's stand-in the first, with
// just `invoke`, an import of `Channel` anywhere would fail to link in every
// file after it — and with just `listen`, `@tauri-apps/api/window`, which
// imports `once` and the rest, in every file loaded after that test.
await import("@tauri-apps/api/core");
await import("@tauri-apps/api/event");

afterEach(cleanup);
