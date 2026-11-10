import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { HttpResponseView, HttpScriptState } from "../lib/httpCommands";

// The HTTP page's state over the backend (docs/45-http-client.md): what is open
// outlives a restart, a tab reads its saved response, the latest send wins, and
// an edit is written as the request object alone once typing stops.

const FILE = {
  info: { name: "Orders API", schema: "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
  item: [
    {
      id: "f",
      name: "Orders",
      item: [{ id: "r1", name: "Create order", request: { method: "POST", url: "{{baseUrl}}/orders", header: [{ key: "Accept", value: "*/*", description: "kept" }] } }],
    },
    { id: "r2", name: "Health", request: "{{baseUrl}}/health" },
  ],
};

const view = (status: number, body = "ok"): HttpResponseView => ({
  at: 1,
  environment: "Stage",
  outcome: { kind: "response", status, headers: [["content-type", "text/plain"]], url: "http://x", elapsedMs: 5 },
  body,
  size: body.length,
  scripts: { off: [], tests: [], console: [], changed: false },
});

let calls: { command: string; args?: Record<string, unknown> }[] = [];
let responses: Record<string, HttpResponseView | null> = {};
let refusals: Record<string, string> = {};
/** Sends kept waiting, oldest first, each let go with what it answers. */
let held: ((v: HttpResponseView) => void)[] | null = null;
let curl: unknown = null;
/** History as the store gives it, the oldest first. */
let log: { id: string }[] = [];
/** The collection's scripts as the backend says, turned on and off by `http_scripts_set`. */
let states: HttpScriptState[] = [];
let scriptsRefused = false;
/** What scripts wrote, as the store gives it back. */
let current: { key: string; value: string; secret: boolean; enabled: boolean }[] = [];
/** Commands kept waiting until the test lets them go, by name. */
let holding = new Set<string>();
let holds: Record<string, (value: unknown) => void> = {};

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (holding.has(command)) return new Promise((resolve) => (holds[command] = resolve));
    if (command === "http_collection_list") return Promise.resolve([{ id: "c", file: structuredClone(FILE), error: null, current }]);
    if (command === "http_environment_list") return Promise.resolve({ environments: [], globals: [] });
    if (command === "http_scripts") return scriptsRefused ? Promise.reject("the settings do not read") : Promise.resolve(structuredClone(states));
    if (command === "http_scripts_set") {
      const hashes = args!.hashes as string[];
      if (hashes.includes("stale")) return Promise.reject("The collection's scripts changed — look at them again.");
      states = states.map((s) => (s.hashes.some((h) => hashes.includes(h)) ? { ...s, on: args!.on as boolean } : s));
      return Promise.resolve(null);
    }
    if (command === "http_response_last") return Promise.resolve(responses[args!.requestId as string] ?? null);
    if (command === "http_parse_curl") return Promise.resolve(curl);
    if (command === "http_history_list") return Promise.resolve(log);
    if (command === "http_import") return Promise.resolve({ kind: "collection", id: "c", name: "Orders API" });
    if (command === "http_send") {
      const refused = refusals[args!.requestId as string];
      if (refused) return Promise.reject(refused);
      if (held) return new Promise((resolve) => held!.push(resolve));
      return Promise.resolve(view(200));
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

mock.module("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve("/picked/shop.postman_collection.json"), save: () => Promise.resolve(null) }));

/** Who listens for `http:changed` now. */
const listeners = new Set<() => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: () => void) => {
    if (channel === "http:changed") listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useHttpClient } = await import("../hooks/useHttpClient");

beforeEach(() => {
  localStorage.clear();
  calls = [];
  responses = {};
  refusals = {};
  held = null;
  states = [];
  scriptsRefused = false;
  current = [];
  holding = new Set();
  holds = {};
  log = [];
  listeners.clear();
});

const called = (command: string) => calls.filter((c) => c.command === command).map((c) => c.args);

async function page() {
  const hook = renderHook(() => useHttpClient());
  await waitFor(() => expect(hook.result.current.collections.length).toBe(1));
  return hook;
}

describe("the HTTP page's state", () => {
  test("after a restart the same tabs are open, and the tab reads its saved response", async () => {
    responses.r1 = view(201, "made");
    const first = await page();
    act(() => first.result.current.open("r1"));
    await waitFor(() => expect(first.result.current.sent.r1?.view?.body).toBe("made"));
    first.unmount();

    calls = [];
    const again = await page();
    expect(again.result.current.tabs).toEqual(["r1"]);
    expect(again.result.current.active).toBe("r1");
    await waitFor(() => expect(again.result.current.sent.r1?.view).toEqual(view(201, "made")));
    expect(called("http_response_last")).toEqual([{ collection: "c", requestId: "r1" }]);
  });

  test("the later send wins: the earlier one's answer, coming after it, is dropped", async () => {
    held = [];
    const hook = await page();
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(held!.length).toBe(1));
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(held!.length).toBe(2));
    expect(hook.result.current.sent.r2?.busy).toBe(true);
    await act(async () => held![1](view(200, "second")));
    await act(async () => held![0](view(500, "first")));
    expect(hook.result.current.sent.r2).toMatchObject({ busy: false, view: { body: "second" } });
  });

  test("each collection's scripts are read with it; a switch turns them and reads them again", async () => {
    states = [
      { item: null, listen: "test", codes: ["x();"], hashes: ["h0"], on: true },
      { item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: false },
    ];
    const hook = await page();
    await waitFor(() => expect(hook.result.current.scripts.c).toEqual(states));
    await act(async () => hook.result.current.turnScripts("c", ["h1"], true));
    expect(called("http_scripts_set")).toEqual([{ collection: "c", hashes: ["h1"], on: true }]);
    expect(hook.result.current.scripts.c.map((s) => s.on)).toEqual([true, true]);
    await act(async () => hook.result.current.turnScripts("c", ["h0"], false));
    expect(hook.result.current.scripts.c.map((s) => s.on)).toEqual([false, true]);
  });

  test("a switch refused says the scripts changed, and what is read again stays", async () => {
    states = [{ item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["stale"], on: false }];
    const hook = await page();
    await act(async () => hook.result.current.turnScripts("c", ["stale"], true));
    expect(hook.result.current.problem).toContain("changed");
    expect(hook.result.current.scripts.c).toEqual(states);
  });

  test("scripts that do not read are said, not taken for none", async () => {
    scriptsRefused = true;
    const hook = await page();
    await waitFor(() => expect(hook.result.current.problem).toBe("The scripts were not read: the settings do not read"));
    scriptsRefused = false;
  });

  test("a send asks nothing about scripts: it goes, and the backend runs what is on", async () => {
    states = [{ item: "r2", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: false }];
    const hook = await page();
    const reads = called("http_scripts").length;
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(hook.result.current.sent.r2?.view?.body).toBe("ok"));
    expect(called("http_send").length).toBe(1);
    expect(called("http_scripts").length).toBe(reads);
  });

  test("a send whose scripts wrote reads the variables again, keeping edits not saved; one that did not, does not", async () => {
    const hook = await page();
    const health = () => hook.result.current.collections[0].items[1];
    const lists = () => called("http_collection_list").length;
    const before = lists();
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(hook.result.current.sent.r2?.view?.body).toBe("ok"));
    expect(lists()).toBe(before);

    act(() => hook.result.current.edit({ ...(health() as never), url: "{{baseUrl}}/typed" }));
    current = [{ key: "token", value: "fresh", secret: false, enabled: true }];
    const wrote = view(200, "wrote");
    wrote.scripts.changed = true;
    held = [];
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(held!.length).toBe(1));
    await act(async () => held![0](wrote));
    await waitFor(() => expect(hook.result.current.collections[0].variables).toEqual([{ key: "token", value: "fresh" }]));
    expect(called("http_environment_list").length).toBeGreaterThan(1);
    expect(health()).toMatchObject({ url: "{{baseUrl}}/typed" });
  });

  test("a send overtaken wrote its variables all the same", async () => {
    held = [];
    const hook = await page();
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(held!.length).toBe(1));
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(held!.length).toBe(2));
    current = [{ key: "token", value: "fresh", secret: false, enabled: true }];
    const wrote = view(200, "first");
    wrote.scripts.changed = true;
    await act(async () => held![0](wrote));
    await waitFor(() => expect(hook.result.current.collections[0].variables.length).toBe(1));
    expect(hook.result.current.sent.r2?.view).toBeUndefined();
  });

  test("a pre-request script that failed keeps what it wrote: the page reads it", async () => {
    refusals.r2 = "A pre-request script failed";
    const hook = await page();
    current = [{ key: "token", value: "fresh", secret: false, enabled: true }];
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(hook.result.current.sent.r2?.notSent).toContain("failed"));
    await waitFor(() => expect(hook.result.current.collections[0].variables.length).toBe(1));
  });

  test("a saved response read after a send answered does not take its place", async () => {
    holding.add("http_response_last");
    const hook = await page();
    act(() => hook.result.current.open("r2"));
    await waitFor(() => expect(holds.http_response_last).toBeDefined());
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(hook.result.current.sent.r2?.view?.body).toBe("ok"));
    await act(async () => holds.http_response_last(view(500, "stale")));
    expect(hook.result.current.sent.r2?.view?.body).toBe("ok");
  });

  test("a refusal before sending is said, and leaves the last response in place", async () => {
    responses.r2 = view(200, "before");
    refusals.r2 = "not found in the collection, the environment or globals: baseUrl";
    const hook = await page();
    act(() => hook.result.current.open("r2"));
    await waitFor(() => expect(hook.result.current.sent.r2?.view?.body).toBe("before"));
    act(() => hook.result.current.send("r2"));
    await waitFor(() => expect(hook.result.current.sent.r2?.notSent).toContain("baseUrl"));
    expect(hook.result.current.sent.r2).toMatchObject({ busy: false, view: { body: "before" } });
  });

  test("an edit is written once typing stops, as the request object alone, its extras kept", async () => {
    const hook = await page();
    const r1 = () => hook.result.current.collections[0].items[0].kind === "folder" && hook.result.current.collections[0].items[0].items[0];
    act(() => hook.result.current.edit({ ...(r1() as never), url: "{{baseUrl}}/orders/1" }));
    act(() => hook.result.current.edit({ ...(r1() as never), url: "{{baseUrl}}/orders/12" }));
    expect(called("http_request_save")).toEqual([]);
    await waitFor(() => expect(called("http_request_save").length).toBe(1));
    expect(called("http_request_save")[0]).toEqual({
      collection: "c",
      requestId: "r1",
      request: { method: "POST", url: "{{baseUrl}}/orders/12", header: [{ key: "Accept", value: "*/*", description: "kept" }] },
    });
  });

  test("a write waits for the one before it: a rename does not reach the backend before the add", async () => {
    holding.add("http_item_add");
    const hook = await page();
    let id: string | null = null;
    act(() => {
      id = hook.result.current.newRequest("c", "New request");
    });
    act(() => hook.result.current.rename(id!, "Ping"));
    await waitFor(() => expect(holds.http_item_add).toBeDefined());
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_item_rename")).toEqual([]);
    await act(async () => holds.http_item_add(null));
    await waitFor(() => expect(called("http_item_rename")).toEqual([{ collection: "c", id, name: "Ping" }]));
  });

  test("an edit not yet written is written when the page closes; a tab's response is read once while it is edited", async () => {
    holding.add("http_response_last");
    const hook = await page();
    act(() => hook.result.current.open("r2"));
    await waitFor(() => expect(holds.http_response_last).toBeDefined());
    // Edited while the read is still on its way: no second read for each edit.
    for (const url of ["/a", "/ab", "/abc"]) act(() => hook.result.current.edit({ ...(hook.result.current.collections[0].items[1] as never), url }));
    expect(called("http_response_last").length).toBe(1);
    await act(async () => holds.http_response_last(view(200, "kept")));
    expect(hook.result.current.sent.r2?.view?.body).toBe("kept");
    hook.unmount();
    await waitFor(() => expect(called("http_request_save")).toEqual([{ collection: "c", requestId: "r2", request: { url: "/abc", method: "GET" } }]));
  });

  test("an import reads the collections back after the edits still waiting are written", async () => {
    const hook = await page();
    act(() => hook.result.current.edit({ ...(hook.result.current.collections[0].items[1] as never), url: "/edited" }));
    await act(() => hook.result.current.importFile());
    const order = calls.map((c) => c.command).filter((c) => c === "http_request_save" || c === "http_collection_list");
    expect(order).toEqual(["http_collection_list", "http_request_save", "http_collection_list"]);
    expect(called("http_import")).toEqual([{ path: "/picked/shop.postman_collection.json" }]);
  });

  test("an edit not yet written goes with its request when that is deleted", async () => {
    const hook = await page();
    const health = hook.result.current.collections[0].items[1];
    act(() => hook.result.current.edit({ ...(health as never), url: "/gone" }));
    act(() => hook.result.current.remove("r2"));
    await waitFor(() => expect(called("http_item_delete").length).toBe(1));
    await new Promise((r) => setTimeout(r, 500));
    expect(called("http_request_save")).toEqual([]);
  });

  test("a new request is added with its name and opened; a deleted folder takes its tabs", async () => {
    const hook = await page();
    act(() => hook.result.current.open("r1"));
    let id: string | null = null;
    act(() => {
      id = hook.result.current.newRequest("f", "Ping");
    });
    expect(hook.result.current.active).toBe(id);
    await waitFor(() => expect(called("http_item_add").length).toBe(1));
    expect(called("http_item_add")[0]).toMatchObject({ collection: "c", parent: "f", item: { id, name: "Ping", request: { method: "GET", url: "" } } });

    act(() => hook.result.current.open("r2"));
    act(() => hook.result.current.remove("f"));
    expect(hook.result.current.tabs).toEqual(["r2"]);
    await waitFor(() => expect(called("http_item_delete")).toEqual([{ collection: "c", id: "f" }]));
    act(() => hook.result.current.remove("c"));
    await waitFor(() => expect(called("http_collection_delete")).toEqual([{ id: "c" }]));
    expect(hook.result.current.tabs).toEqual([]);
  });

  test("an agent's send shows in the log while the log is on screen, and nothing listens after", async () => {
    const hook = renderHook(({ shown }) => useHttpClient(shown), { initialProps: { shown: false } });
    await waitFor(() => expect(hook.result.current.collections.length).toBe(1));
    expect(listeners.size).toBe(0);
    expect(called("http_history_list").length).toBe(0);

    log = [{ id: "a" }];
    hook.rerender({ shown: true });
    await waitFor(() => expect(hook.result.current.history?.map((e) => e.id)).toEqual(["a"]));
    expect(listeners.size).toBe(1);

    log = [{ id: "a" }, { id: "b" }];
    await act(async () => listeners.forEach((told) => told()));
    await waitFor(() => expect(hook.result.current.history?.map((e) => e.id)).toEqual(["b", "a"]));

    hook.rerender({ shown: false });
    await waitFor(() => expect(listeners.size).toBe(0));
  });

  test("a pasted curl becomes the request, its name and place kept", async () => {
    curl = { method: "PUT", header: [{ key: "X-A", value: "1" }], url: "https://api.test/v1/items?x=1", body: { mode: "raw", raw: "{}", options: { raw: { language: "json" } } } };
    const hook = await page();
    const health = hook.result.current.collections[0].items[1];
    await act(() => hook.result.current.pasteCurl(health as never, "curl -X PUT …"));
    expect(hook.result.current.collections[0].items[1]).toMatchObject({
      id: "r2",
      name: "Health",
      method: "PUT",
      url: "https://api.test/v1/items?x=1",
      headers: [{ key: "X-A", value: "1" }],
      body: { mode: "raw", language: "json", raw: "{}" },
    });
  });
});
