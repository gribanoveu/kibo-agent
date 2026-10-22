import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";

// The Artifacts page's wrappers (docs/38-artifacts.md): the signals it reads
// again on — no channel of its own — where a file loads from, and Save as
// going through the system's dialog.

let calls: { command: string; args?: Record<string, unknown> }[] = [];
let chosen: string | null = null;
let offered: unknown;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Map<string, Set<(message: { payload: unknown }) => void>>();
const emit = (channel: string, payload: unknown) => listeners.get(channel)?.forEach((handler) => handler({ payload }));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (!listeners.has(channel)) listeners.set(channel, new Set());
    listeners.get(channel)!.add(handler);
    return Promise.resolve(() => listeners.get(channel)!.delete(handler));
  },
}));

mock.module("@tauri-apps/plugin-dialog", () => ({
  save: (options: unknown) => {
    offered = options;
    return Promise.resolve(chosen);
  },
  open: () => Promise.resolve(null),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { artifactSaveAs, artifactUrl, onArtifactsChanged } = await import("../lib/artifacts");

beforeEach(() => {
  calls = [];
  chosen = null;
  offered = undefined;
});

const turn = (type: string, payload: unknown) => ({ turnId: "t1", seq: 1, round: 0, type, payload });

describe("onArtifactsChanged", () => {
  test("says so on what changes the list, from any turn, and on nothing else", async () => {
    let said = 0;
    const stop = await onArtifactsChanged(() => said++);

    emit("meeting:event", { kind: "transcribed", id: "m" });
    emit("meeting:event", { kind: "ended" });
    expect(said).toBe(2);
    emit("meeting:event", { kind: "progress", id: "m", name: "call.wav", stage: "speech", done: 1, total: 3 });
    emit("meeting:event", { kind: "summaryDelta", id: "m", text: "…" });
    emit("meeting:event", { kind: "recording", id: "r", seconds: 1, mic: 0, call: 0 });
    expect(said).toBe(2);

    emit("chat:turn-event", turn("toolResult", { id: "c1", result: { result: "presented", path: "index.html", title: "T", version: 1 } }));
    emit("chat:turn-event", { ...turn("toolResult", { id: "c2", changes: [{ path: "/p/files/a.css", before: { kind: "absent" }, after: { kind: "unstored" } }] }), turnId: "another" });
    expect(said).toBe(4);
    emit("chat:turn-event", turn("toolResult", { id: "c3", result: { result: "read" }, changes: [] }));
    emit("chat:turn-event", turn("toolResult", { id: "c4", error: "no such file" }));
    emit("chat:turn-event", turn("delta", { delta: "presented" }));
    expect(said).toBe(4);

    stop();
    emit("meeting:event", { kind: "ended" });
    emit("chat:turn-event", turn("toolResult", { id: "c5", result: { result: "presented" } }));
    expect(said).toBe(4);
    expect([...listeners.values()].every((set) => set.size === 0)).toBe(true);
  });
});

describe("artifactUrl", () => {
  test("a prototype's file from its working folder, a meeting's recording, and nothing for what reads as text", () => {
    expect(artifactUrl("prototypes", "c1", "assets/our logo.png", false)).toBe("artifact://localhost/c1/files/assets/our%20logo.png");
    expect(artifactUrl("prototypes", "c1", "index.html", true)).toBe("http://artifact.localhost/c1/files/index.html");
    expect(artifactUrl("meetings", "m1", "audio.wav", false)).toBe("meeting://localhost/m1");
    expect(artifactUrl("meetings", "m1", "audio.wav", true)).toBe("http://meeting.localhost/m1");
    expect(artifactUrl("meetings", "m1", "notes.md", false)).toBeNull();
  });
});

describe("artifactSaveAs", () => {
  test("asks the system's dialog where, then saves there", async () => {
    chosen = "/Users/me/Desktop/Планёрка.md";
    expect(await artifactSaveAs("meetings", "m1", "notes.md", "Планёрка")).toBe(chosen);
    expect(offered).toMatchObject({ defaultPath: "Планёрка.md", filters: [{ extensions: ["md"] }] });
    expect(calls).toEqual([{ command: "artifact_save_as", args: { kind: "meetings", id: "m1", path: "notes.md", dest: chosen } }]);

    calls = [];
    chosen = "/tmp/logo.png";
    await artifactSaveAs("prototypes", "c1", "assets/logo.png");
    expect(offered).toMatchObject({ defaultPath: "logo.png" });
    expect(calls[0].args).toMatchObject({ path: "assets/logo.png", dest: chosen });
  });

  test("a cancelled dialog saves nothing", async () => {
    chosen = null;
    expect(await artifactSaveAs("meetings", "m1", "notes.md")).toBeNull();
    expect(calls).toEqual([]);
  });
});
