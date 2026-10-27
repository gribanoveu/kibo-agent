import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { MongoConnection } from "../lib/mongo";

// The MongoDB page: a connection added with its connection string — which the
// page never gets back, only the address masked — its databases and
// collections in a tree, a collection's documents found, edited and deleted.

let connections: MongoConnection[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let refusals: Record<string, string> = {};
let findNext: number | null = 7;
// What a find returns; `held` keeps the next one waiting until it is let go.
let findDocs: unknown[];
let held: (() => void) | null = null;
let holdNext = false;

const value = (text: string, bson: string) => ({ kind: "value", text, bson }) as const;
const DOC = {
  id: { $oid: "65f0a1b2c3d4e5f6a7b8c9d0" },
  text: '{\n  _id: ObjectId("65f0a1b2c3d4e5f6a7b8c9d0"),\n  name: "Ann"\n}',
  fields: [
    ["_id", value('ObjectId("65f0a1b2c3d4e5f6a7b8c9d0")', "ObjectId")],
    ["name", value('"Ann"', "String")],
    ["tags", { kind: "array", items: [value('"vip"', "String"), { kind: "object", fields: [["n", value("NumberLong(5)", "Int64")]] }] }],
  ],
};

const BOB = { id: { $oid: "65f0a1b2c3d4e5f6a7b8c9d1" }, text: '{ name: "Bob" }', fields: [["name", value('"Bob"', "String")]] };

/** Documents of one shape: their texts as long as each other's. */
const named = (n: number, name: string) => ({ id: { $numberInt: String(n) }, text: `{ _id: ${n}, name: "${name}" }`, fields: [["_id", value(String(n), "Int32")], ["name", value(`"${name}"`, "String")]] });
const [ONE, TWO, SIX] = [named(1, "one"), named(2, "two"), named(6, "six")];

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
      case "mongo_find": {
        const page = { documents: structuredClone(findDocs), next: findNext };
        if (!holdNext) return Promise.resolve(page);
        holdNext = false;
        return new Promise((resolve) => (held = () => resolve(page)));
      }
      case "mongo_find_more":
        return Promise.resolve({ documents: [BOB], next: null });
      case "mongo_count":
        return Promise.resolve(41);
      case "mongo_filter_add": {
        const { filter, path, text, exclude } = args as { filter: string; path: string[]; text: string; exclude: boolean };
        return Promise.resolve(`${filter || "{}"} + ${path.join(".")}${exclude ? " != " : " = "}${text}`);
      }
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
const { isHistory, pageRange, ranAgo, remember, withoutId } = await import("../lib/mongo");
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
  findNext = 7;
  findDocs = [DOC];
  held = null;
  holdNext = false;
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
    expect(within(screen.getByText('"Ann"').closest("li")!).getByText("String")).toBeTruthy();
    // No total until asked: on a view, counting runs its whole pipeline.
    expect(screen.getByText("1–1")).toBeTruthy();
    expect(sent("mongo_count")).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Count" }));
    await settle();
    expect(sent("mongo_count")).toEqual([{ target: { connection: "c1", database: "shop", collection: "orders" }, filter: "" }]);
    expect(screen.getByText("1–1 of 41")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Count" })).toBeNull();
  });

  test("a document's fields are a line each, what nests closed until opened or all expanded", async () => {
    await openOrders();
    const tags = screen.getByRole("button", { name: /tags.*Array \(2\)/ });
    expect(tags.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText('"vip"')).toBeNull();
    fireEvent.click(tags);
    expect(screen.getByText('"vip"')).toBeTruthy();
    fireEvent.click(tags);
    expect(screen.queryByText('"vip"')).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(screen.getByText('"vip"')).toBeTruthy();
    expect(screen.getByText("NumberLong(5)")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(screen.queryByText('"vip"')).toBeNull();
  });

  test("a field double-clicked puts the card in edit mode; Save sends what changed, all at once", async () => {
    await openOrders();
    fireEvent.doubleClick(screen.getByText('ObjectId("65f0a1b2c3d4e5f6a7b8c9d0")'));
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();

    fireEvent.doubleClick(screen.getByText('"Ann"'));
    const name = screen.getByRole("textbox", { name: "Edit name" }) as HTMLInputElement;
    expect(name.value).toBe('"Ann"');
    expect(screen.getByText("Double-click a value to change it; + on an object or array adds to it")).toBeTruthy();
    fireEvent.change(name, { target: { value: '"Bea"' } });

    // Opened under a toggle that "Expand all" remounts: the draft above stays.
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect((screen.getByRole("textbox", { name: "Edit name" }) as HTMLInputElement).value).toBe('"Bea"');
    fireEvent.doubleClick(screen.getByText("NumberLong(5)"));
    fireEvent.change(screen.getByRole("textbox", { name: "Edit tags.1.n" }), { target: { value: "NumberLong(6)" } });
    fireEvent.doubleClick(screen.getByText('"vip"'));
    expect(screen.getByText("2 fields changed")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await settle();
    expect(sent("mongo_set_fields")).toEqual([
      {
        target: { connection: "c1", database: "shop", collection: "orders" },
        id: DOC.id,
        changes: [
          { path: ["tags", "1", "n"], text: "NumberLong(6)" },
          { path: ["name"], text: '"Bea"' },
        ],
      },
    ]);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(sent("mongo_find")).toHaveLength(2);
  });

  test("fields and items added go out with the changes: a new name, or the index after the array's last", async () => {
    await openOrders();
    // Adding is editing: outside it a value's + is the filter's, and nothing adds.
    expect(screen.queryByRole("button", { name: "Add item to tags" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add field" })).toBeNull();
    fireEvent.doubleClick(screen.getByText('"Ann"'));
    expect(screen.queryByRole("button", { name: "Filter by name" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Add item to tags" }));
    fireEvent.change(screen.getByRole("textbox", { name: "New item of tags" }), { target: { value: '"x"' } });
    fireEvent.click(screen.getByRole("button", { name: "Add item to tags" }));
    const items = screen.getAllByRole("textbox", { name: "New item of tags" });
    expect(items.map((item) => item.closest("li")!.querySelector(".mongo-entry-key")!.textContent)).toEqual(["2", "3"]);
    fireEvent.change(items[1], { target: { value: "7" } });
    // A removed one leaves no gap: the next takes its index.
    fireEvent.click(screen.getAllByRole("button", { name: "Remove the new item of tags" })[0]);
    expect(screen.getByRole("textbox", { name: "New item of tags" }).closest("li")!.querySelector(".mongo-entry-key")!.textContent).toBe("2");

    fireEvent.click(screen.getByRole("button", { name: "Add field to tags.1" }));
    fireEvent.change(screen.getByRole("textbox", { name: "New field name in tags.1" }), { target: { value: "m" } });
    fireEvent.change(screen.getByRole("textbox", { name: "New field value in tags.1" }), { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "Add field" }));
    fireEvent.change(screen.getByRole("textbox", { name: "New field name in the document" }), { target: { value: "note" } });
    const note = screen.getByRole("textbox", { name: "New field value in the document" });
    fireEvent.change(note, { target: { value: '"hi"' } });
    expect(screen.getByText("3 fields changed")).toBeTruthy();
    fireEvent.keyDown(note, { code: "Enter" });
    await settle();
    expect(sent("mongo_set_fields")[0]!.changes).toEqual([
      { path: ["tags", "2"], text: "7" },
      { path: ["tags", "1", "m"], text: "1" },
      { path: ["note"], text: '"hi"' },
    ]);
    expect(screen.queryByRole("textbox", { name: "New item of tags" })).toBeNull();
  });

  test("Cancel or Escape leaves edit mode with nothing sent, and Enter saves", async () => {
    await openOrders();
    fireEvent.doubleClick(screen.getByText('"Ann"'));
    fireEvent.change(screen.getByRole("textbox", { name: "Edit name" }), { target: { value: '"Bea"' } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("textbox", { name: "Edit name" })).toBeNull();
    expect(screen.getByText('"Ann"')).toBeTruthy();

    fireEvent.doubleClick(screen.getByText('"Ann"'));
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Edit name" }), { code: "Escape" });
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(sent("mongo_set_fields")).toEqual([]);

    fireEvent.doubleClick(screen.getByText('"Ann"'));
    const name = screen.getByRole("textbox", { name: "Edit name" });
    fireEvent.change(name, { target: { value: '"Cy"' } });
    fireEvent.keyDown(name, { code: "Enter" });
    await settle();
    expect(sent("mongo_set_fields")).toHaveLength(1);
  });

  test("a save the server refuses stays in edit mode with the reason on the card", async () => {
    refusals.mongo_set_fields = "name: line 1, column 1: expected a value";
    await openOrders();
    fireEvent.doubleClick(screen.getByText('"Ann"'));
    fireEvent.change(screen.getByRole("textbox", { name: "Edit name" }), { target: { value: "Bea" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await settle();
    expect(screen.getByRole("textbox", { name: "Edit name" }).closest("article")!.textContent).toContain("name: line 1, column 1: expected a value");
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();
  });

  test("a field's + and − add its condition to the filter, which is found only once Find is pressed", async () => {
    await openOrders();
    const filter = screen.getByRole("combobox", { name: "Filter" }) as HTMLInputElement;
    fireEvent.change(filter, { target: { value: "{ a: 1 }" } });
    fireEvent.click(screen.getByRole("button", { name: "Filter by name" }));
    await settle();
    expect(filter.value).toBe('{ a: 1 } + name = "Ann"');

    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    // An array's item by the array's name: any of its items matches.
    fireEvent.click(screen.getByRole("button", { name: "Filter out tags.n" }));
    await settle();
    expect(sent("mongo_filter_add")[1]).toEqual({ filter: '{ a: 1 } + name = "Ann"', path: ["tags", "n"], text: "NumberLong(5)", exclude: true });
    expect(filter.value).toBe('{ a: 1 } + name = "Ann" + tags.n != NumberLong(5)');
    expect(sent("mongo_find")).toHaveLength(1);

    refusals.mongo_filter_add = "the filter: line 1, column 3: expected ':'";
    fireEvent.click(screen.getByRole("button", { name: "Filter by tags" }));
    await settle();
    expect(screen.getByText("the filter: line 1, column 3: expected ':'")).toBeTruthy();
    expect(filter.value).toBe('{ a: 1 } + name = "Ann" + tags.n != NumberLong(5)');
  });

  test("a value copies as it would be pasted — a string without its quotes", async () => {
    const copied: string[] = [];
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text: string) => void copied.push(text) } });
    await openOrders();
    expect(screen.getByText("3 fields")).toBeTruthy();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy name" })));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy _id" })));
    expect(copied).toEqual(["Ann", 'ObjectId("65f0a1b2c3d4e5f6a7b8c9d0")']);
  });

  test("the filter helps: brackets close, the documents' fields are suggested, Tab or Enter takes one", async () => {
    await openOrders();
    const filter = screen.getByRole("combobox", { name: "Filter" }) as HTMLInputElement;
    filter.focus();
    fireEvent.keyDown(filter, { key: "{", code: "BracketLeft" });
    expect(filter.value).toBe("{}");
    expect(filter.selectionStart).toBe(1);
    fireEvent.keyDown(filter, { key: "Backspace", code: "Backspace" });
    expect(filter.value).toBe("");

    fireEvent.change(filter, { target: { value: "{ na" } });
    const list = screen.getByRole("listbox", { name: "Suggestions for the filter" });
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual(["nameStringTab"]);
    expect(filter.getAttribute("aria-expanded")).toBe("true");
    fireEvent.keyDown(filter, { key: "Tab", code: "Tab" });
    expect(filter.value).toBe('{ name: ""');
    expect(filter.selectionStart).toBe(9);
    expect(screen.queryByRole("listbox", { name: "Suggestions for the filter" })).toBeNull();

    // Escape closes the list; Enter then finds instead of picking.
    fireEvent.change(filter, { target: { value: "{ ta" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["tagsArrayTab", "tags.nInt64"]);
    fireEvent.keyDown(filter, { key: "Escape", code: "Escape" });
    expect(screen.queryByRole("listbox", { name: "Suggestions for the filter" })).toBeNull();
    fireEvent.keyDown(filter, { key: "Enter", code: "Enter" });
    await settle();
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "{ ta" } });

    // With the list open, Enter takes the suggestion and finds nothing.
    fireEvent.change(filter, { target: { value: "{ $exi" } });
    fireEvent.keyDown(filter, { key: "Enter", code: "Enter" });
    expect(filter.value).toBe("{ $exists: true");
    expect(sent("mongo_find")).toHaveLength(2);
  });

  test("an example picked replaces the filter, its field selected to type over", async () => {
    await openOrders();
    const filter = screen.getByRole("combobox", { name: "Filter" }) as HTMLInputElement;
    fireEvent.click(screen.getAllByRole("button", { name: "Examples" })[0]);
    fireEvent.click(screen.getByRole("option", { name: /Not equal to/ }));
    expect(filter.value).toBe('{ field: { $ne: "value" } }');
    expect([filter.selectionStart, filter.selectionEnd]).toEqual([2, 7]);
  });

  test("a query run is kept in the collection's history, and picked from it runs again", async () => {
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(screen.getByText("No queries run yet")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "History" }));

    const filter = screen.getByRole("combobox", { name: "Filter" }) as HTMLInputElement;
    fireEvent.change(filter, { target: { value: "{ name: 'Ann' }" } });
    fireEvent.keyDown(filter, { code: "Enter" });
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(filter.value).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "History" }));
    // Under the query, when it ran, counted as the menu opened.
    fireEvent.click(screen.getByRole("option", { name: "{ name: 'Ann' } now" }));
    await settle();
    expect(filter.value).toBe("{ name: 'Ann' }");
    expect(sent("mongo_find")).toHaveLength(3);
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "{ name: 'Ann' }", skip: 0 } });
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
    // The next page goes on from the find left open, not from a new one.
    expect(sent("mongo_find_more")).toEqual([{ next: 7 }]);
    expect(sent("mongo_find")).toHaveLength(2);
    expect(screen.getByText('"Bob"')).toBeTruthy();
    expect(screen.getByText("21–21")).toBeTruthy();
    const next = screen.getByRole("button", { name: "Next page" }) as HTMLButtonElement;
    expect(next.disabled).toBe(true);

    // Back and forth over what was read asks for nothing.
    fireEvent.click(screen.getByRole("button", { name: "Previous page" }));
    expect(screen.getByText('"Ann"')).toBeTruthy();
    expect((screen.getByRole("button", { name: "Previous page" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(next);
    await settle();
    expect(screen.getByText('"Bob"')).toBeTruthy();
    expect(sent("mongo_find_more")).toHaveLength(1);
    expect(sent("mongo_find")).toHaveLength(2);
  });

  test("a find left open is closed when another replaces it or the tab closes", async () => {
    await openOrders();
    expect(sent("mongo_find_close")).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    expect(sent("mongo_find_close")).toEqual([{ next: 7 }]);
    fireEvent.click(screen.getByRole("button", { name: "Close shop.orders" }));
    await settle();
    expect(sent("mongo_find_close")).toEqual([{ next: 7 }, { next: 7 }]);
  });

  test("the last page has no next, and the page waits under a spinner while it is read", async () => {
    findNext = null;
    page();
    await settle();
    fireEvent.click(screen.getByRole("button", { name: /shop/ }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "orders" }));
    expect(screen.getByRole("status").textContent).toContain("Loading documents");
    await settle();
    expect(screen.queryByRole("status")).toBeNull();
    expect((screen.getByRole("button", { name: "Next page" }) as HTMLButtonElement).disabled).toBe(true);
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

  test("an insert starts with an _id made now, a fresh one each time", async () => {
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "Insert document" }));
    const text = (dialog().getByRole("textbox") as HTMLTextAreaElement).value;
    const id = /^\{\n {2}_id: ObjectId\("([0-9a-f]{24})"\),\n {2}\n\}$/.exec(text)?.[1];
    expect(id).toBeTruthy();
    expect(Math.abs(parseInt(id!.slice(0, 8), 16) - Date.now() / 1000)).toBeLessThan(5);
    fireEvent.click(dialog().getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Insert document" }));
    expect((dialog().getByRole("textbox") as HTMLTextAreaElement).value).not.toBe(text);
  });

  test("a refused insert says why in the dialog, closed with its ×, and leaves with the dialog", async () => {
    refusals.mongo_insert = "E11000 duplicate key error (DuplicateKey)";
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "Insert document" }));
    fireEvent.click(dialog().getByRole("button", { name: "Insert" }));
    await settle();
    expect(dialog().getByRole("alert").textContent).toContain("E11000 duplicate key error");
    fireEvent.click(within(dialog().getByRole("alert")).getByRole("button", { name: "Close" }));
    expect(dialog().queryByRole("alert")).toBeNull();

    fireEvent.click(dialog().getByRole("button", { name: "Insert" }));
    await settle();
    fireEvent.click(dialog().getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
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

  test("two collections open in tabs, each keeping its own query", async () => {
    await openOrders();
    // The other tab's query bar is there too, hidden.
    const filter = () => screen.getAllByLabelText("Filter").find((el) => !el.closest("[hidden]")) as HTMLInputElement;
    fireEvent.change(filter(), { target: { value: "{ a: 1 }" } });
    fireEvent.click(screen.getByRole("button", { name: /big_orders/ }));
    await settle();
    expect(screen.getAllByRole("tab", { name: /orders/ }).map((t) => t.textContent)).toEqual(["orders", "big_orders"]);
    expect(filter().value).toBe("");
    expect(sent("mongo_find").at(-1)).toMatchObject({ target: { collection: "big_orders" } });

    fireEvent.click(screen.getByRole("tab", { name: "orders" }));
    expect(filter().value).toBe("{ a: 1 }");
    fireEvent.click(screen.getByRole("button", { name: "Close shop.orders" }));
    expect(screen.getAllByRole("tab", { name: /orders/ }).map((t) => t.textContent)).toEqual(["big_orders"]);
    expect(screen.getByRole("tab", { name: "big_orders" }).getAttribute("aria-selected")).toBe("true");
  });

  test("a find with a projection shows parts of documents: no Edit or Clone, which would write the part back", async () => {
    await openOrders();
    expect(screen.getByRole("button", { name: "Edit document" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Options" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Project" }), { target: { value: "{ name: 1 }" } });
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    expect(screen.queryByRole("button", { name: "Edit document" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Clone document" })).toBeNull();
    expect(screen.getByRole("button", { name: "Delete document" })).toBeTruthy();
    // A field changed alone is still safe: only it is set.
    fireEvent.doubleClick(screen.getByText('"Ann"'));
    expect(screen.getByRole("textbox", { name: "Edit name" })).toBeTruthy();
  });

  test("an edit begun stays with its document when the list is found again around it", async () => {
    findDocs = [ONE, TWO, SIX];
    await openOrders();
    fireEvent.doubleClick(screen.getByText('"two"'));
    fireEvent.change(screen.getByRole("textbox", { name: "Edit name" }), { target: { value: '"2"' } });
    // The first one deleted: the rest move up a place.
    findDocs = [TWO, SIX];
    fireEvent.click(screen.getAllByRole("button", { name: "Delete document" })[0]);
    fireEvent.click(dialog().getByRole("button", { name: "Delete" }));
    await settle();
    expect(sent("mongo_delete").map((a) => a!.id)).toEqual([ONE.id]);
    const editing = screen.getByRole("textbox", { name: "Edit name" }).closest("article")!;
    expect(within(editing).queryByText('"six"')).toBeNull();
    fireEvent.click(within(editing).getByRole("button", { name: "Save" }));
    await settle();
    expect(sent("mongo_set_fields").map((a) => a!.id)).toEqual([TWO.id]);
  });

  test("a query typed and not run stays typed while the list is found again, and the page size stays chosen", async () => {
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: "20 per page" }));
    fireEvent.click(screen.getByRole("option", { name: "50 per page" }));
    await settle();
    const filter = screen.getByRole("combobox", { name: "Filter" }) as HTMLInputElement;
    fireEvent.change(filter, { target: { value: "{ name: 'A' }" } });
    fireEvent.click(screen.getByRole("button", { name: "Find again" }));
    await settle();
    expect(filter.value).toBe("{ name: 'A' }");
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "", limit: 50 } });
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "{ name: 'A' }", limit: 50 } });
  });

  test("a refused find leaves the last one in place: what is shown is found again, and counted, by its own filter", async () => {
    await openOrders();
    const filter = screen.getByRole("combobox", { name: "Filter" }) as HTMLInputElement;
    fireEvent.change(filter, { target: { value: "{ a: }" } });
    refusals.mongo_find = "the filter: line 1, column 6: expected a value";
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    expect(filter.value).toBe("{ a: }");
    delete refusals.mongo_find;
    fireEvent.click(screen.getByRole("button", { name: "Count" }));
    await settle();
    expect(sent("mongo_count").at(-1)).toMatchObject({ filter: "" });
    fireEvent.click(screen.getByRole("button", { name: "Delete document" }));
    fireEvent.click(dialog().getByRole("button", { name: "Delete" }));
    await settle();
    expect(sent("mongo_find").at(-1)).toMatchObject({ request: { filter: "", skip: 0 } });
  });

  test("a find that comes back after its tab closed, or after another find, has its cursor closed", async () => {
    await openOrders();
    holdNext = true;
    findNext = 8;
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    // Another find while it waits — Enter is not held back by the busy buttons: the waiting one is stale when it comes.
    findNext = 9;
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Filter" }), { code: "Enter" });
    await settle();
    expect(sent("mongo_find_close")).toEqual([{ next: 7 }]);
    held!();
    await settle();
    expect(sent("mongo_find_close")).toEqual([{ next: 7 }, { next: 8 }]);

    holdNext = true;
    findNext = 10;
    fireEvent.click(screen.getByRole("button", { name: "Find" }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Close shop.orders" }));
    await settle();
    held!();
    await settle();
    expect(sent("mongo_find_close")).toEqual([{ next: 7 }, { next: 8 }, { next: 9 }, { next: 10 }]);
  });

  test("another connection chosen reads nothing of the last one's tabs, and leaves its history alone", async () => {
    connections = [...connections, { id: "c2", name: "Other", address: "mongodb://other" }];
    const theirs = [{ filter: "{ b: 2 }", projection: "", sort: "", at: 1 }];
    localStorage.setItem("mongo.history.c2.shop.orders", JSON.stringify(theirs));
    await openOrders();
    fireEvent.click(screen.getByRole("button", { name: /Local/ }));
    fireEvent.click(screen.getByRole("option", { name: /Other/ }));
    await settle();
    const toOther = calls.filter((c) => (c.args?.target as { connection?: string } | undefined)?.connection === "c2");
    expect(toOther).toEqual([]);
    expect(JSON.parse(localStorage.getItem("mongo.history.c2.shop.orders")!)).toEqual(theirs);
    expect(screen.queryByRole("tab", { name: "orders" })).toBeNull();
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
    expect(pageRange(20, 20, 1234)).toBe("21–40 of 1,234");
    expect(pageRange(0, 0, 0)).toBe("0 of 0");
    expect(pageRange(20, 20, null)).toBe("21–40");
    expect(pageRange(0, 0, null)).toBe("0");
    expect(withoutId('{\n  _id: 1,\n  a: 2\n}')).toBe("{\n  a: 2\n}");
    expect(withoutId('{\n  a: 2,\n  _id: 1\n}')).toBe("{\n  a: 2\n}");
    // An `_id` that is a document of its own is left for the user to change.
    expect(withoutId('{\n  _id: {\n    k: 1\n  },\n  a: 2\n}')).toBe('{\n  _id: {\n    k: 1\n  },\n  a: 2\n}');
  });

  test("a query history: newest on top with when it ran, no repeats, nothing empty, ten at most", () => {
    const q = (filter: string, sort = "", at = 1) => ({ filter, projection: "", sort, at });
    expect(remember([], q("  "), 5)).toEqual([]);
    expect(remember([q("{ a: 1 }")], q(" { b: 1 } "), 5)).toEqual([q("{ b: 1 }", "", 5), q("{ a: 1 }")]);
    // Run again, it moves up with its new time.
    expect(remember([q("{ b: 1 }"), q("{ a: 1 }")], q("{ a: 1 }"), 7)).toEqual([q("{ a: 1 }", "", 7), q("{ b: 1 }")]);
    // A sort alone is a query; the same filter with another sort is another one.
    expect(remember([q("{ a: 1 }")], q("{ a: 1 }", "{ a: -1 }"))).toHaveLength(2);
    const full = Array.from({ length: 10 }, (_, i) => q(`{ n: ${i} }`));
    const next = remember(full, q("{ n: 99 }"), 1);
    expect(next).toHaveLength(10);
    expect(next[0]).toEqual(q("{ n: 99 }"));
    expect(next[9]).toEqual(q("{ n: 8 }"));
    expect(isHistory([q("x")])).toBe(true);
    // Kept by a build that did not note the time: not read.
    expect(isHistory([{ filter: "x", projection: "", sort: "" }])).toBe(false);
    expect(isHistory("x")).toBe(false);
  });

  test("when a query ran, said as people say it", () => {
    const now = 1_000_000_000;
    expect(ranAgo(now, now)).toBe("now");
    expect(ranAgo(now + 500, now)).toBe("now");
    expect(ranAgo(now - 30_000, now)).toBe("30 seconds ago");
    expect(ranAgo(now - 59_000, now)).toBe("59 seconds ago");
    expect(ranAgo(now - 60_000, now)).toBe("1 minute ago");
    expect(ranAgo(now - 5 * 60_000, now)).toBe("5 minutes ago");
    expect(ranAgo(now - 2 * 3_600_000, now)).toBe("2 hours ago");
    expect(ranAgo(now - 86_400_000, now)).toBe("yesterday");
    expect(ranAgo(now - 3 * 86_400_000, now)).toBe("3 days ago");
  });
});
