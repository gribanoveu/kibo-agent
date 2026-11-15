import { afterAll, afterEach, expect, mock, test } from "bun:test";

// The scenes the model painted reach the window through `useScene`: listed,
// each body read, and drawn once chosen (docs/55-frost-scenes.md, SC-2).

type Row = { id: string; title: string; created: number };
let rows: Row[] = [];
let bodies: Record<string, string> = {};
let refuseDelete = false;
let refuseList = false;
const calls: { command: string; args: unknown }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: { id: string }) => {
    calls.push({ command, args });
    if (command === "scenes_list") return refuseList ? Promise.reject("the scenes' store failed: bad index") : Promise.resolve(rows);
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
  calls.length = 0;
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
