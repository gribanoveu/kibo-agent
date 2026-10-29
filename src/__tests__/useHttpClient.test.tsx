import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { HttpResponseView } from "../lib/httpCommands";

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
});

let calls: { command: string; args?: Record<string, unknown> }[] = [];
let responses: Record<string, HttpResponseView | null> = {};
let refusals: Record<string, string> = {};
/** Sends kept waiting, oldest first, each let go with what it answers. */
let held: ((v: HttpResponseView) => void)[] | null = null;
let curl: unknown = null;
/** Commands kept waiting until the test lets them go, by name. */
let holding = new Set<string>();
let holds: Record<string, (value: unknown) => void> = {};

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (holding.has(command)) return new Promise((resolve) => (holds[command] = resolve));
    if (command === "http_collection_list") return Promise.resolve([{ id: "c", file: structuredClone(FILE), error: null }]);
    if (command === "http_environment_list") return Promise.resolve({ environments: [], globals: [] });
    if (command === "http_response_last") return Promise.resolve(responses[args!.requestId as string] ?? null);
    if (command === "http_parse_curl") return Promise.resolve(curl);
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
  holding = new Set();
  holds = {};
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
    act(() => hook.result.current.send("r2"));
    expect(hook.result.current.sent.r2?.busy).toBe(true);
    await act(async () => held![1](view(200, "second")));
    await act(async () => held![0](view(500, "first")));
    expect(hook.result.current.sent.r2).toMatchObject({ busy: false, view: { body: "second" } });
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
