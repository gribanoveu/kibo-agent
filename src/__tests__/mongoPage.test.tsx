import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { MongoConnection } from "../lib/mongo";

// The MongoDB page: a connection added with its connection string — which the
// page never gets back, only the address masked — its databases and
// collections in a tree, a collection's documents found, edited and deleted.

let connections: MongoConnection[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let refusals: Record<string, string> = {};

const DOC = { id: { $oid: "65f0a1b2c3d4e5f6a7b8c9d0" }, text: '{\n  _id: ObjectId("65f0a1b2c3d4e5f6a7b8c9d0"),\n  name: "Ann"\n}' };

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (refusals[command]) return Promise.reject(refusals[command]);
    switch (command) {
      case "mongo_connections":
        return Promise.resolve(structuredClone(connections));
      case "mongo_connection_save": {
        const draft = args!.draft as { id?: string; name: string; uri?: string };
        if (draft.id) connections = connections.map((c) => (c.id === draft.id ? { ...c, name: draft.name } : c));
        else connections = [...connections, { id: `c${connections.length + 1}`, name: draft.name, address: draft.uri!.replace(/:[^:@/]+@/, ":•••@") }];
        return Promise.resolve(structuredClone(connections));
      }
      case "mongo_connection_delete":
        connections = connections.filter((c) => c.id !== args!.id);
        return Promise.resolve(structuredClone(connections));
      case "mongo_connection_test":
        return Promise.resolve("8.0.4");
      case "mongo_databases":
        return Promise.resolve([{ name: "shop", sizeOnDisk: 2_621_440 }]);
      case "mongo_collections":
        return Promise.resolve([
          { name: "orders", kind: "collection" },
          { name: "big_orders", kind: "view" },
        ]);
      case "mongo_find":
        return Promise.resolve({ documents: [DOC], total: 41 });
      case "mongo_indexes":
        return Promise.resolve([{ name: "_id_", keys: "{ _id: 1 }", unique: false, sparse: false, ttlSeconds: null, partial: null }]);
      default:
        return Promise.resolve(null);
    }
  },
  transformCallback: (callback: unknown) => callback,
}));

mock.module("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { MongoPage } = await import("../components/MongoPage");
const { diskSize, pageRange, withoutId } = await import("../lib/mongo");
const { IS_MAC } = await import("../lib/shortcuts");
const MOD = IS_MAC ? { metaKey: true } : { ctrlKey: true };
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const page = () =>
  render(<MongoPage openPane={() => {}} openPage={() => {}} openChat={() => {}} removeChat={() => Promise.resolve(true)} chats={[]} />);
const sent = (command: string) => calls.filter((c) => c.command === command).map((c) => c.args);
const dialog = () => within(screen.getByRole("dialog"));

beforeEach(() => {
  localStorage.clear();
  calls = [];
  refusals = {};
  connections = [{ id: "c1", name: "Local", address: "mongodb://root:•••@localhost:27017" }];
});

const openOrders = async () => {
  page();
  await settle();
  fireEvent.click(screen.getByRole("button", { name: /shop/ }));
  await settle();
  fireEvent.click(screen.getByRole("button", { name: "orders" }));
  await settle();
};

describe("connections", () => {
  test("none yet: a connection added from the welcome, its string sent once and shown masked", async () => {
    connections = [];
    page();
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
    fireEvent.change(dialog().getByLabelText("Name"), { target: { value: "Prod" } });
    fireEvent.change(dialog().getByLabelText(/Connection string/), { target: { value: "mongodb://admin:secret@db:27017" } });
    fireEvent.click(dialog().getByRole("button", { name: "Test" }));
    await settle();
    expect(dialog().getByText("Connected — MongoDB 8.0.4")).toBeTruthy();
    fireEvent.click(dialog().getByRole("button", { name: "Save" }));
    await settle();

    expect(sent("mongo_connection_save")).toEqual([{ draft: { id: undefined, name: "Prod", uri: "mongodb://admin:secret@db:27017" } }]);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: /Prod/ }).getAttribute("title")).toBe("mongodb://admin:•••@db:27017");
    expect(sent("mongo_databases")).toContainEqual({ connection: "c1" });
  });

  test("an edit with the string left empty keeps the saved one: no uri is sent", async () => {
    page();
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect((dialog().getByLabelText(/leave empty to keep/) as HTMLInputElement).placeholder).toBe("mongodb://root:•••@localhost:27017");
    fireEvent.change(dialog().getByLabelText("Name"), { target: { value: "Laptop" } });
    fireEvent.click(dialog().getByRole("button", { name: "Save" }));
    await settle();
    expect(sent("mongo_connection_save")).toEqual([{ draft: { id: "c1", name: "Laptop", uri: undefined } }]);
  });

  test("deleting one asks first", async () => {
    page();
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(dialog().getByRole("button", { name: "Delete" }));
    expect(screen.getByRole("dialog", { name: "Delete “Local”?" })).toBeTruthy();
    fireEvent.click(within(screen.getByRole("dialog", { name: "Delete “Local”?" })).getByRole("button", { name: "Delete" }));
    await settle();
    expect(sent("mongo_connection_delete")).toEqual([{ id: "c1" }]);
    expect(screen.getByRole("button", { name: "Add connection" })).toBeTruthy();
  });
});

describe("a collection", () => {
  test("its databases, then its collections, then its first page of documents", async () => {
    await openOrders();
    expect(screen.getByText("2.5 MB")).toBeTruthy();
    expect(screen.getByText("view")).toBeTruthy();
    expect(sent("mongo_find")).toEqual([{ target: { connection: "c1", database: "shop", collection: "orders" }, request: { filter: "", projection: "", sort: "", skip: 0, limit: 20 } }]);
    expect(screen.getByText(/name: "Ann"/)).toBeTruthy();
    expect(screen.getByText("1–1 of 41")).toBeTruthy();
  });

  test("a filter is found with Enter, and the next page skips what was shown", async () => {
    await openOrders();
    const filter = screen.getByLabelText("Filter");
    fireEvent.change(filter, { target: { value: "{ name: /^A/ }" } });
    fireEvent.keyDown(filter, { code: "Enter" });
    await settle();
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "{ name: /^A/ }", skip: 0 } });
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    await settle();
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "{ name: /^A/ }", skip: 20, limit: 20 } });
  });

  test("the backend's word on a bad query is shown as it is", async () => {
    await openOrders();
    refusals.mongo_find = "the filter: line 1, column 9: expected ':'";
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    expect(screen.getByText("the filter: line 1, column 9: expected ':'")).toBeTruthy();
  });

  test("a document edited is replaced by its _id, and found again", async () => {
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "Edit document" }));
    const editor = dialog().getByRole("textbox");
    expect((editor as HTMLTextAreaElement).value).toBe(DOC.text);
    fireEvent.change(editor, { target: { value: DOC.text.replace("Ann", "Bea") } });
    fireEvent.keyDown(editor, { code: "Enter", ...MOD });
    await settle();
    expect(sent("mongo_replace")).toEqual([{ target: { connection: "c1", database: "shop", collection: "orders" }, id: DOC.id, text: DOC.text.replace("Ann", "Bea") }]);
    expect(sent("mongo_find")).toHaveLength(2);
  });

  test("a refused edit stays open with the reason", async () => {
    await openOrders();
    refusals.mongo_replace = "the _id cannot change — insert a copy and delete this one instead";
    fireEvent.click(screen.getByRole("button", { name: "Edit document" }));
    fireEvent.click(dialog().getByRole("button", { name: "Replace" }));
    await settle();
    expect(dialog().getByText(/the _id cannot change/)).toBeTruthy();
  });

  test("a clone starts without the _id, and is inserted", async () => {
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "Clone document" }));
    expect((dialog().getByRole("textbox") as HTMLTextAreaElement).value).toBe('{\n  name: "Ann"\n}');
    fireEvent.click(dialog().getByRole("button", { name: "Insert" }));
    await settle();
    expect(sent("mongo_insert")).toEqual([{ target: { connection: "c1", database: "shop", collection: "orders" }, text: '{\n  name: "Ann"\n}' }]);
  });

  test("a document is deleted only after asking", async () => {
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "Delete document" }));
    expect(sent("mongo_delete")).toEqual([]);
    fireEvent.click(dialog().getByRole("button", { name: "Delete" }));
    await settle();
    expect(sent("mongo_delete")).toEqual([{ target: { connection: "c1", database: "shop", collection: "orders" }, id: DOC.id }]);
  });

  test("indexes are read when their tab opens, and _id_ cannot be dropped", async () => {
    await openOrders();
    expect(sent("mongo_indexes")).toEqual([]);
    fireEvent.click(screen.getByRole("tab", { name: "Indexes" }));
    await settle();
    expect(sent("mongo_indexes")).toHaveLength(1);
    expect(screen.getByText("{ _id: 1 }")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Drop index _id_" })).toBeNull();
  });
});

describe("helpers", () => {
  test("sizes, ranges and a clone's text", () => {
    expect(diskSize(512)).toBe("512 B");
    expect(diskSize(2_621_440)).toBe("2.5 MB");
    expect(diskSize(50 * 1024 ** 3)).toBe("50 GB");
    expect(pageRange(20, 20, 1234)).toBe("21–40 of 1,234");
    expect(pageRange(0, 0, 0)).toBe("0 of 0");
    expect(withoutId('{\n  _id: 1,\n  a: 2\n}')).toBe("{\n  a: 2\n}");
    expect(withoutId('{\n  a: 2,\n  _id: 1\n}')).toBe("{\n  a: 2\n}");
    // An `_id` that is a document of its own is left for the user to change.
    expect(withoutId('{\n  _id: {\n    k: 1\n  },\n  a: 2\n}')).toBe('{\n  _id: {\n    k: 1\n  },\n  a: 2\n}');
  });
});
