import { afterAll, afterEach, expect, mock, test } from "bun:test";

// The scenes the model painted reach the window through `useScene`: listed,
// each body read, and drawn once chosen (docs/55-frost-scenes.md, SC-2).

type Row = { id: string; title: string; created: number };
let rows: Row[] = [];
let bodies: Record<string, string> = {};
let refuseDelete = false;
let refuseList = false;
// While set, a list read waits for it, and answers with the list as it was when asked.
let listGate: Promise<void> | null = null;
let refuseGated = false;
const calls: { command: string; args: unknown }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: { id: string }) => {
    calls.push({ command, args });
    if (command === "scenes_list") {
      if (refuseList) return Promise.reject("the scenes' store failed: bad index");
      const asked = rows;
      if (listGate && refuseGated) return listGate.then(() => Promise.reject("the scenes' store failed: busy"));
      return listGate ? listGate.then(() => asked) : Promise.resolve(asked);
    }
    if (command === "scene_read") return args.id in bodies ? Promise.resolve(bodies[args.id]) : Promise.reject(`no scene ${args.id}`);
    if (command === "scene_delete") {
      if (refuseDelete) return Promise.reject("the scenes' store failed: disk full");
      rows = rows.filter((r) => r.id !== args.id);
      delete bodies[args.id];
      return Promise.resolve();
    }
    return Promise.reject(`unexpected ${command}`);
  },
}));

// The turn's channel and the features', as the backend would speak on them.
const listeners = new Map<string, (event: { payload: unknown }) => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler);
    return Promise.resolve(() => listeners.delete(name));
  },
}));
const say = (name: string, payload: unknown = null) => act(() => listeners.get(name)?.({ payload }));
const toolResult = (result: unknown) => say("chat:turn-event", { type: "toolResult", payload: { id: "call-1", result } });

const host = window as unknown as Record<string, unknown>;
host.__TAURI_INTERNALS__ = {};
afterAll(() => delete host.__TAURI_INTERNALS__);

const { useScene } = await import("../hooks/useScene");
const { act, renderHook, waitFor } = await import("@testing-library/react");

afterEach(() => {
  localStorage.clear();
  rows = [];
  bodies = {};
  refuseDelete = false;
  refuseList = false;
  listGate = null;
  refuseGated = false;
  calls.length = 0;
  listeners.clear();
});

const paint = (id: string, title: string, body: string) => {
  rows = [{ id, title, created: rows.length }, ...rows];
  bodies[id] = body;
};

test("a painted scene, once chosen, is what the window draws", async () => {
  paint("a", "Forest", "<i class=forest></i>");
  paint("b", "Lake", "<i class=lake></i>");
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.made.map((s) => s.title)).toEqual(["Lake", "Forest"]));
  act(() => result.current.choose({ kind: "made", id: "a" }));
  expect(result.current).toMatchObject({ active: { kind: "made", id: "a" }, body: "<i class=forest></i>" });
});

test("a chosen painted scene shows once the list is read, at the next launch too", async () => {
  paint("a", "Forest", "<i class=forest></i>");
  localStorage.setItem("kibo-scene", JSON.stringify({ kind: "made", id: "a" }));
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.body).toBe("<i class=forest></i>"));
});

test("a scene whose body cannot be read is left out, the rest stay", async () => {
  paint("a", "Forest", "<i></i>");
  rows = [{ id: "gone", title: "Gone", created: 9 }, ...rows];
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.made.map((s) => s.id)).toEqual(["a"]));
});

test("deleting the scene that shows leaves no scene, and the list is read again", async () => {
  paint("a", "Forest", "<i></i>");
  paint("b", "Lake", "<i></i>");
  localStorage.setItem("kibo-scene", JSON.stringify({ kind: "made", id: "b" }));
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.active).toEqual({ kind: "made", id: "b" }));
  await act(() => result.current.remove("b"));
  expect(calls.filter((c) => c.command === "scene_delete").map((c) => c.args)).toEqual([{ id: "b" }]);
  expect(result.current.made.map((s) => s.id)).toEqual(["a"]);
  expect(result.current).toMatchObject({ active: null, body: null, error: null });
});

test("a delete that fails says why and keeps the scene", async () => {
  paint("a", "Forest", "<i></i>");
  refuseDelete = true;
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.made).toHaveLength(1));
  await act(() => result.current.remove("a"));
  expect(result.current.error).toBe("the scenes' store failed: disk full");
  expect(result.current.made.map((s) => s.id)).toEqual(["a"]);
});

test("a list that cannot be read says why, until a read succeeds", async () => {
  paint("a", "Forest", "<i></i>");
  refuseList = true;
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.error).toBe("the scenes' store failed: bad index"));
  expect(result.current.made).toEqual([]);
  refuseList = false;
  await act(() => result.current.remove("gone"));
  expect(result.current.error).toBeNull();
  expect(result.current.made.map((s) => s.id)).toEqual(["a"]);
});

test("a scene the model paints is read and chosen, and the theme is left alone", async () => {
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(listeners.has("chat:turn-event")).toBe(true));
  paint("new", "Snowy forest", "<i class=snow></i>");
  await toolResult({ result: "scenePainted", id: "new", title: "Snowy forest" });
  await waitFor(() => expect(result.current).toMatchObject({ active: { kind: "made", id: "new" }, body: "<i class=snow></i>" }));
  expect(result.current.made.map((s) => s.title)).toEqual(["Snowy forest"]);
  expect(localStorage.getItem("kibo-theme")).toBeNull();
});

test("another tool's result changes nothing", async () => {
  localStorage.setItem("kibo-scene", JSON.stringify({ kind: "builtin", id: "snowfall" }));
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(listeners.has("chat:turn-event")).toBe(true));
  const reads = calls.length;
  await toolResult({ result: "presented", path: "index.html", title: "Shop", version: 1 });
  await toolResult({ result: "scenePainted" });
  await toolResult({ result: "background", id: "7" });
  expect(calls.length).toBe(reads);
  expect(result.current.active).toEqual({ kind: "builtin", id: "snowfall" });
});

test("a feature's change reads the painted scenes again — its data may be gone", async () => {
  paint("a", "Forest", "<i></i>");
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.made).toHaveLength(1));
  rows = [];
  await say("features:changed");
  await waitFor(() => expect(result.current.made).toEqual([]));
});

test("the listeners go with the hook", async () => {
  const { unmount } = renderHook(() => useScene());
  await waitFor(() => expect(listeners.size).toBe(2));
  unmount();
  await waitFor(() => expect(listeners.size).toBe(0));
});

test("of two scenes painted in a row, the last is chosen and kept, whichever read ends last", async () => {
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(listeners.has("chat:turn-event")).toBe(true));
  let release = () => {};
  listGate = new Promise<void>((done) => (release = done));
  paint("a", "Forest", "<i class=a></i>");
  await toolResult({ result: "scenePainted", id: "a", title: "Forest" });
  listGate = null;
  paint("b", "Lake", "<i class=b></i>");
  await toolResult({ result: "scenePainted", id: "b", title: "Lake" });
  await waitFor(() => expect(result.current.active).toEqual({ kind: "made", id: "b" }));
  await act(async () => release());
  await act(() => new Promise<void>((done) => setTimeout(done, 5)));
  expect(result.current.active).toEqual({ kind: "made", id: "b" });
  expect(result.current.made.map((s) => s.id)).toEqual(["b", "a"]);
});

test("a read that fails after a later one succeeded says nothing", async () => {
  paint("a", "Forest", "<i></i>");
  const { result } = renderHook(() => useScene());
  await waitFor(() => expect(result.current.made).toHaveLength(1));
  let release = () => {};
  listGate = new Promise<void>((done) => (release = done));
  refuseGated = true;
  await say("features:changed");
  listGate = null;
  await say("features:changed");
  await act(async () => release());
  await act(() => new Promise<void>((done) => setTimeout(done, 5)));
  expect(result.current.error).toBeNull();
  expect(result.current.made).toHaveLength(1);
});
