import { afterAll, beforeEach, describe, expect, mock, spyOn, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { currentCompletions } from "@codemirror/autocomplete";
import type { HttpHistoryEntry, HttpResponseView, HttpScriptDraft, HttpScriptState } from "../lib/httpCommands";
import type { Environment } from "../lib/httpClient";

// The HTTP page on its stores: a collection, its requests and an environment
// with a secret the page has only ever seen masked.

const FILE = {
  info: { name: "Orders API", schema: "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
  variable: [{ key: "orderId", value: "9" }],
  item: [
    { id: "r1", name: "Create order", event: [{ listen: "prerequest", script: { exec: ["pm.environment.set('t', 1);"] } }], request: { method: "POST", url: "{{baseUrl}}/orders" } },
    { id: "r2", name: "Health", request: "{{baseUrl}}/health" },
    { id: "r4", name: "Me", request: { method: "GET", url: "{{baseUrl}}/me", auth: { type: "bearer", bearer: [{ key: "token", value: "{{apiToken}}" }] } } },
    { id: "r3", name: "Upload", request: { method: "POST", url: "{{baseUrl}}/files", body: { mode: "graphql", graphql: {} } } },
  ],
};

const STAGE: Environment = {
  id: "stage",
  name: "Stage",
  variables: [
    { key: "baseUrl", value: "https://stage.test", secret: false, enabled: true },
    { key: "token", value: "•••", secret: true, enabled: true },
  ],
};

const ENTRY: HttpHistoryEntry = {
  id: "h1",
  at: 1,
  environment: "Stage",
  agentChat: null,
  templateUrl: "{{baseUrl}}/health",
  requestId: "r2",
  request: { method: "GET", url: "https://stage.test/health", headers: [["Authorization", "Bearer •••"]], body: null },
  outcome: { kind: "response", status: 200, headers: [["content-type", "application/json"]], url: "https://stage.test/health", elapsedMs: 4 },
  responseBody: '{"ok": true}',
};

let calls: { command: string; args?: Record<string, unknown> }[] = [];
/** The environment as the backend keeps it: saved by the form, written by a script. */
let stage: Environment = structuredClone(STAGE);
let holdSends = false;
/** The collection's scripts as the backend says, turned on and off by `http_scripts_set`. */
let states: HttpScriptState[] = [];
/** Script saves kept waiting, each let go by the test; null lets them through. */
let scriptSaves: (() => void)[] | null = null;
let jar: { domain: string; path: string; name: string; expiresAt?: number | null }[] = [];
/** The chats' drafts, as `http_drafts` lists them. */
let drafts: HttpScriptDraft[] = [];
/** A chat's post-response draft of "Create order", run on a send of it. */
const DRAFT: HttpScriptDraft = {
  collection: "c",
  request: "r1",
  name: "Orders API / Create order",
  listen: "test",
  chat: "chat-1",
  chatTitle: "Check orders",
  environment: "Stage",
  code: "pm.test('made', () => pm.response.to.have.status(201));",
  at: 1,
  report: {
    exchange: "h9",
    ranOn: "POST https://stage.test/orders → 200",
    masked: ["$.token"],
    supplied: ["orderId"],
    hidden: [],
    tests: [{ name: "made", passed: false, skipped: false, error: "expected 200 to be 201" }],
    console: [],
    error: null,
    skipped: false,
    writes: [{ scope: "environment", key: "orderId", value: "7" }],
    sends: [
      { method: "GET", url: "https://elsewhere.test/x", masked: false, foreign: true },
      { method: "POST", url: "https://stage.test/sign", masked: true, foreign: false },
      { method: "GET", url: "https://stage.test/ok", masked: false, foreign: false },
    ],
    request: [],
    requestError: null,
  },
};
let answer: HttpResponseView;
const reported = (scripts: Partial<HttpResponseView["scripts"]>): HttpResponseView => ({
  at: 1,
  environment: null,
  outcome: { kind: "response", status: 200, headers: [], url: "http://x", elapsedMs: 5 },
  body: "ok",
  size: 2,
  scripts: { off: [], tests: [], console: [], changed: false, ...scripts },
});

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "http_collection_list") return Promise.resolve([{ id: "c", file: structuredClone(FILE), error: null }]);
    if (command === "http_environment_list") return Promise.resolve({ environments: [structuredClone(stage)], globals: [] });
    if (command === "http_history_list") return Promise.resolve([ENTRY]);
    if (command === "http_scripts") return Promise.resolve(structuredClone(states));
    if (command === "http_script_save" && scriptSaves) return new Promise((done) => scriptSaves!.push(() => done(null)));
    if (command === "http_script_save") {
      // As the backend: an edit keeps a script on as it was, a new one is off (decision 5), blank takes it away.
      const { requestId, listen, code } = args as { requestId: string; listen: "prerequest" | "test"; code: string };
      const was = states.find((s) => s.item === requestId && s.listen === listen);
      states = states.filter((s) => s !== was);
      if (code.trim()) states.push({ item: requestId, listen, codes: [code], hashes: [`h:${code}`], on: was?.on ?? false });
      return Promise.resolve(null);
    }
    if (command === "http_drafts") return Promise.resolve(structuredClone(drafts));
    if (command === "http_draft_apply" || command === "http_draft_reject") {
      // As the backend: the draft shown, by its key and code — one written since is another.
      const shown = args!.draft as Pick<HttpScriptDraft, "collection" | "request" | "listen" | "chat" | "code">;
      const found = drafts.find((d) => d.collection === shown.collection && d.request === shown.request && d.listen === shown.listen && d.chat === shown.chat && d.code === shown.code);
      if (command === "http_draft_apply" && !found) return Promise.reject("The draft changed or is gone — look at it again.");
      drafts = drafts.filter((d) => d !== found);
      if (command === "http_draft_apply") {
        states = states.filter((s) => !(s.item === shown.request && s.listen === shown.listen));
        states.push({ item: shown.request, listen: shown.listen, codes: [shown.code], hashes: [`h:${shown.code}`], on: true });
      }
      return Promise.resolve(null);
    }
    if (command === "http_scripts_set") {
      const hashes = args!.hashes as string[];
      if (hashes.includes("stale")) return Promise.reject("The collection's scripts changed — look at them again.");
      states = states.map((s) => (s.hashes.some((h) => hashes.includes(h)) ? { ...s, on: args!.on as boolean } : s));
      return Promise.resolve(null);
    }
    if (command === "http_cookies") return Promise.resolve(jar);
    if (command === "http_cookie_delete") {
      jar = jar.filter((c) => c.name !== args!.name);
      return Promise.resolve(null);
    }
    if (command === "http_cookies_clear") {
      jar = [];
      return Promise.resolve(null);
    }
    if (command === "http_collection_new") return Promise.resolve({ id: "c2", file: { info: { name: args!.name }, item: [] }, error: null });
    if (command === "http_send") {
      if (args!.requestId === "r3") return Promise.reject("a graphql body is kept from Postman and not sent yet");
      return holdSends ? new Promise<HttpResponseView>(() => {}) : Promise.resolve(answer);
    }
    if (command === "http_environment_secret") return Promise.resolve("hunter2");
    if (command === "http_environment_save") {
      const env = args!.environment as Environment;
      if (env.trustedCert === "junk") return Promise.reject("the certificate: no certificate in it");
      stage = { ...env, variables: env.variables.map((v) => (v.secret ? { ...v, value: "•••" } : v)) };
      return Promise.resolve(null);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

/** Who listens on `http:changed`: a test tells them the backend changed. */
const told = new Set<() => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (_channel: string, handler: () => void) => {
    told.add(handler);
    return Promise.resolve(() => told.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { HttpPage } = await import("../components/HttpPage");
const { IS_MAC } = await import("../lib/shortcuts");

const MOD = IS_MAC ? { metaKey: true } : { ctrlKey: true };
const sendButton = () => screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;
const called = (command: string) => calls.filter((c) => c.command === command).map((c) => c.args);

/** The page with its collection read, `tab` open in a tab. */
async function shown(tab?: string) {
  if (tab) {
    localStorage.setItem("http.tabs", JSON.stringify([tab]));
    localStorage.setItem("http.active", JSON.stringify(tab));
  }
  render(<HttpPage />);
  await screen.findByText("Orders API");
}

beforeEach(() => {
  localStorage.clear();
  calls = [];
  stage = structuredClone(STAGE);
  holdSends = false;
  states = [];
  scriptSaves = null;
  jar = [];
  drafts = [];
  answer = reported({});
});

describe("the HTTP page", () => {
  test("the jar lists what each domain set, and a cookie or all of them go", async () => {
    jar = [
      { domain: "a.test", path: "/", name: "sid", expiresAt: null },
      { domain: "b.test", path: "/app", name: "theme", expiresAt: 1_900_000_000_000 },
    ];
    await shown();
    fireEvent.click(screen.getByRole("button", { name: "Cookies" }));
    expect((await screen.findByText("a.test")) !== null).toBe(true);
    expect(screen.getByText("When cleared") !== null).toBe(true);
    expect(screen.getByText("/app") !== null).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Delete sid" }));
    await waitFor(() => expect(called("http_cookie_delete")).toEqual([{ domain: "a.test", path: "/", name: "sid" }]));
    await waitFor(() => expect(screen.queryByText("a.test") === null).toBe(true));

    fireEvent.click(screen.getByRole("button", { name: "Clear all" }));
    expect((await screen.findByText(/No cookies yet/)) !== null).toBe(true);
    expect(called("http_cookies_clear").length).toBe(1);
  });

  test("oauth2 is set up on the Auth tab and goes with the send", async () => {
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: "Auth" }));
    fireEvent.click(screen.getByRole("button", { name: /Inherit/i }));
    fireEvent.click(await screen.findByRole("option", { name: /OAuth 2.0/ }));
    expect(screen.getByText("Client credentials") !== null).toBe(true);
    fireEvent.change(screen.getByLabelText("Token URL"), { target: { value: "{{baseUrl}}/token" } });
    fireEvent.change(screen.getByLabelText("Client ID"), { target: { value: "app" } });
    expect(screen.queryByLabelText("Username") === null).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Client credentials" }));
    fireEvent.click(await screen.findByRole("option", { name: "Password" }));
    expect(screen.getByLabelText("Username") !== null).toBe(true);

    fireEvent.click(sendButton());
    await waitFor(() => expect(called("http_send").length).toBe(1));
    const sent = called("http_send")[0]!.request as { auth: { type: string; oauth2: { key: string; value: string }[] } };
    expect(sent.auth.type).toBe("oauth2");
    expect(Object.fromEntries(sent.auth.oauth2.map((r) => [r.key, r.value]))).toEqual({ grant_type: "password_credentials", accessTokenUrl: "{{baseUrl}}/token", clientId: "app" });
  });

  test("a form is typed on the Body tab and goes with the send", async () => {
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: "Body" }));
    fireEvent.click(screen.getByRole("radio", { name: "form-data" }));
    fireEvent.change(screen.getByLabelText("Field"), { target: { value: "name" } });
    fireEvent.click(sendButton());
    await waitFor(() => expect(called("http_send").length).toBe(1));
    expect(called("http_send")[0]!.request).toMatchObject({ body: { mode: "formdata", formdata: [{ key: "name", value: "", type: "text" }] } });
  });

  test("renaming selects the name once, and renames the collection on the backend", async () => {
    await shown();
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /Rename/ }));
    const input = screen.getByLabelText("Name") as HTMLInputElement;
    const select = spyOn(HTMLInputElement.prototype, "select");
    fireEvent.change(input, { target: { value: "Orders API v2" } });
    fireEvent.change(input, { target: { value: "Orders API v23" } });
    expect(select).not.toHaveBeenCalled();
    select.mockRestore();
    expect(input.value).toBe("Orders API v23");
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(screen.getByText("Orders API v23")).toBeDefined();
    await waitFor(() => expect(called("http_item_rename")).toEqual([{ collection: "c", id: "c", name: "Orders API v23" }]));
  });

  test("a new collection or request is made only once its name is confirmed", async () => {
    await shown();
    fireEvent.click(screen.getByLabelText("New collection"));
    expect(screen.getByRole("dialog", { name: "New collection" })).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /New request/ }));
    expect(screen.getByRole("dialog", { name: "New request" })).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_collection_new")).toEqual([]);
    expect(called("http_item_add")).toEqual([]);

    fireEvent.click(screen.getByLabelText("New collection"));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Billing" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByText("Billing")).toBeDefined();
    expect(called("http_collection_new")).toEqual([{ name: "Billing" }]);
    expect(called("http_item_rename")).toEqual([]);
  });

  test("what is open in the tree is open again when the page comes back", async () => {
    await shown();
    expect(screen.queryByText("Health") === null).toBe(true);
    fireEvent.click(screen.getByText("Orders API"));
    expect(screen.queryByText("Health") !== null).toBe(true);
    cleanup();
    render(<HttpPage />);
    expect((await screen.findByText("Health")) !== null).toBe(true);
    fireEvent.click(screen.getByText("Orders API"));
    expect(screen.queryByText("Health") === null).toBe(true);
    expect(JSON.parse(localStorage.getItem("http.expanded") ?? "null")).toEqual([]);
  });

  test("a tab picked shows its request in the tree, its collection opened", async () => {
    await shown("r2");
    const row = () => document.querySelector('[data-tree-request="r2"]');
    expect(row()).toBe(null);
    fireEvent.click(screen.getByRole("tab", { name: /Health/ }));
    expect(row()?.className).toContain(" on");
    expect(JSON.parse(localStorage.getItem("http.expanded") ?? "null")).toEqual(["c"]);
  });

  test("a request moves into a folder picked from its menu, with nowhere to go said", async () => {
    await shown();
    fireEvent.click(screen.getByText("Orders API"));
    fireEvent.click(screen.getByLabelText("Actions for Health"));
    fireEvent.click(screen.getByRole("option", { name: /Move to/ }));
    expect(screen.getByText(/There is nowhere else to put it/) !== null).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /New folder/ }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Refunds" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /New folder/ }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Returns" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    fireEvent.click(screen.getByLabelText("Actions for Returns"));
    fireEvent.click(screen.getByRole("option", { name: /New folder/ }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Inner" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(called("http_item_add")).toHaveLength(3));
    const folder = (called("http_item_add")[1]!.item as { id: string }).id;

    fireEvent.click(screen.getByLabelText("Actions for Health"));
    fireEvent.click(screen.getByRole("option", { name: /Move to/ }));
    const move = screen.getByRole("button", { name: "Move" }) as HTMLButtonElement;
    expect(move.disabled).toBe(true);
    fireEvent.click(screen.getByRole("option", { name: "Orders API / Returns" }));
    expect(screen.getByRole("option", { selected: true }).getAttribute("aria-label")).toBe("Orders API / Returns");
    // Where it is now is shown, and not a place to pick.
    expect((screen.getByRole("option", { name: "Orders API" }) as HTMLButtonElement).disabled).toBe(true);
    // Folded at first but for the way to where it is; all opened; a place folded by its own
    // arrow takes what it holds, however deep, and opens again; all folded.
    const seen = (label: string) => screen.queryByRole("option", { name: label }) !== null;
    expect(seen("Orders API / Returns / Inner")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    expect(seen("Orders API / Returns / Inner")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Collapse Orders API" }));
    expect([seen("Orders API / Returns"), seen("Orders API / Returns / Inner")]).toEqual([false, false]);
    fireEvent.click(screen.getByRole("button", { name: "Expand Orders API" }));
    expect([seen("Orders API / Returns"), seen("Orders API / Returns / Inner")]).toEqual([true, true]);
    fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(seen("Orders API / Returns")).toBe(false);
    fireEvent.click(move);
    await waitFor(() => expect(called("http_item_move")).toEqual([{ collection: "c", id: "r2", to: "c", parent: folder }]));
    // Shown in it at once: the folder opened, the request inside.
    expect(document.querySelector(`[aria-label="Actions for Returns"]`)?.closest("li")?.textContent?.includes("Health")).toBe(true);
    // Back into the collection itself: no folder named.
    fireEvent.click(screen.getByLabelText("Actions for Health"));
    fireEvent.click(screen.getByRole("option", { name: /Move to/ }));
    fireEvent.click(screen.getByRole("option", { name: "Orders API" }));
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    await waitFor(() => expect(called("http_item_move")[1]).toEqual({ collection: "c", id: "r2", to: "c", parent: null }));
  });

  test("a new folder is made in a collection only once its name is confirmed, and is empty", async () => {
    await shown();
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /New folder/ }));
    expect(screen.getByRole("dialog", { name: "New folder" })).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_item_add")).toEqual([]);

    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /New folder/ }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Refunds" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByText("Refunds")).toBeDefined();
    await waitFor(() => expect(called("http_item_add")).toHaveLength(1));
    const [added] = called("http_item_add") as { collection: string; parent: string | null; item: { id: string; name: string; item: unknown[] } }[];
    expect([added.collection, added.parent, added.item.name, added.item.item]).toEqual(["c", null, "Refunds", []]);
    expect(added.item.id).toBeTruthy();
    // A folder holds one too; a request does not.
    fireEvent.click(screen.getByLabelText("Actions for Refunds"));
    expect(screen.getByRole("option", { name: /New folder/ })).toBeDefined();
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    fireEvent.click(screen.getByLabelText("Actions for Health"));
    expect(screen.queryByRole("option", { name: /New folder/ })).toBeNull();
  });

  test("⌘Enter under a dialog is the dialog's; outside one it sends the open request", async () => {
    holdSends = true;
    await shown("r2");
    fireEvent.click(screen.getByLabelText("Actions for Orders API"));
    fireEvent.click(screen.getByRole("option", { name: /Rename/ }));
    fireEvent.keyDown(window, { code: "Enter", key: "Enter", ...MOD });
    expect(sendButton().disabled).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(window, { code: "Enter", key: "Enter", ...MOD });
    expect(sendButton().disabled).toBe(true);
    await waitFor(() => expect(called("http_send")).toEqual([{ collection: "c", requestId: "r2", request: "{{baseUrl}}/health", environment: null }]));
  });

  test("a script off is marked in the tree and on its tab, and its switch turns it on", async () => {
    states = [{ item: "r1", listen: "prerequest", codes: ["first();", "second();"], hashes: ["h1"], on: false }];
    await shown("r1");
    fireEvent.click(screen.getByText("Orders API"));
    // Read after the collections: the tree is drawn before its marks are.
    await waitFor(() => expect(screen.queryByLabelText("Postman scripts, off") !== null).toBe(true));
    expect(screen.getByLabelText("Scripts off") !== null).toBe(true);
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    expect(screen.getByText("Off — does not run") !== null).toBe(true);
    // Every script of the kind, as the backend runs them one after the other.
    expect(document.querySelector(".cm-content")?.textContent).toBe("first();second();");
    await act(async () => fireEvent.click(screen.getByRole("switch", { name: "Run Pre-request" })));
    // A query, not a get: a get that fails prints the whole page, which takes minutes here.
    await waitFor(() => expect(screen.queryByText("On — runs on Send") !== null).toBe(true));
    expect(called("http_scripts_set")).toEqual([{ collection: "c", hashes: ["h1"], on: true }]);
    expect(screen.getByLabelText("Postman scripts, on") !== null).toBe(true);
    expect(screen.getByLabelText("Scripts on") !== null).toBe(true);
    // Sending asks nothing: what is off stays off, and what is on runs.
    await act(async () => fireEvent.click(sendButton()));
    await waitFor(() => expect(called("http_send").length).toBe(1));
  });

  test("a chat's draft is marked in the tree and opens its tab; moved, it shows what it replaces first and is on", async () => {
    states = [
      { item: "r1", listen: "prerequest", codes: ["first();"], hashes: ["h1"], on: true },
      { item: "r1", listen: "test", codes: ["old();"], hashes: ["h2"], on: false },
    ];
    drafts = [DRAFT];
    await shown("r1");
    // Said on the collection while it is closed, and on the request once open.
    await waitFor(() => expect(screen.queryAllByLabelText("A script draft from a chat is waiting").length).toBe(1));
    fireEvent.click(screen.getByText("Orders API"));
    await waitFor(() => expect(screen.queryAllByLabelText("A script draft from a chat is waiting").length).toBe(2));
    expect(screen.getByLabelText("A draft waiting") !== null).toBe(true);
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    // Opened on the draft's script, not on the first one there is.
    const segments = within(screen.getByRole("tablist", { name: "Scripts, in the order they run" }));
    expect(segments.getByRole("tab", { selected: true }).textContent).toBe("Post-response");
    const block = within(screen.getByRole("region", { name: "Draft from “Check orders”" }));
    expect(block.getByText("Run on POST https://stage.test/orders → 200 with nothing sent. It does not run on Send until you move it into the request.") !== null).toBe(true);
    expect(block.getByText("expected 200 to be 201") !== null).toBe(true);
    // Every send, and louder the one that asks to be read, for either reason.
    const warned = within(block.getByRole("list", { name: "Read before moving" }));
    expect(warned.getAllByRole("listitem").map((w) => [w.textContent, w.className])).toEqual([
      ["Sends GET https://elsewhere.test/x — a host none of the collections' requests goes to", "http-draft-warning"],
      ["Sends POST https://stage.test/sign — a hidden value goes in it", "http-draft-warning"],
      ["Sends GET https://stage.test/ok", ""],
      ["Masked (•••) in the send it ran on — a test of these fails though the API is right: $.token", ""],
      ["Run with values the chat gave, not the environment's: orderId", ""],
    ]);
    expect(block.getByText("Would set: environment orderId = 7") !== null).toBe(true);
    expect(document.querySelector(".http-code-box .cm-content")?.textContent).toBe("old();");
    // Only under the script it is a draft of.
    fireEvent.click(segments.getByRole("tab", { name: "Pre-request" }));
    expect(screen.queryByRole("region", { name: "Draft from “Check orders”" }) === null).toBe(true);
    fireEvent.click(segments.getByRole("tab", { name: /Post-response/ }));

    fireEvent.click(screen.getByRole("button", { name: "Move into the request…" }));
    const shownDialog = screen.getByRole("dialog", { name: "Move the draft into the request's post-response script?" });
    const dialog = within(shownDialog);
    // What it takes the place of, and what goes in: the words of a line changed are marked one by one.
    const flat = (text: string) => text.replace(/\s+/g, "");
    const diff = flat(shownDialog.querySelector(".diff-view")?.textContent ?? "");
    expect(diff).toContain("-old();");
    expect(diff).toContain(`+${flat(DRAFT.code)}`);
    expect(called("http_draft_apply")).toEqual([]);
    await act(async () => fireEvent.click(dialog.getByRole("button", { name: "Move and turn on" })));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Draft from “Check orders”" }) === null).toBe(true));
    expect(called("http_draft_apply")).toEqual([{ draft: { collection: "c", request: "r1", listen: "test", chat: "chat-1", code: DRAFT.code } }]);
    await waitFor(() => expect(screen.queryByText("On — runs on Send") !== null).toBe(true));
    expect(document.querySelector(".http-code-box .cm-content")?.textContent).toBe(DRAFT.code);
    expect(screen.queryByLabelText("A script draft from a chat is waiting") === null).toBe(true);
    expect(called("http_script_save")).toEqual([]);
  });

  test("what was typed into the script is written before the draft takes its place, and the draft is what shows", async () => {
    states = [{ item: "r1", listen: "test", codes: ["old();"], hashes: ["h2"], on: false }];
    drafts = [DRAFT];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Draft from “Check orders”" }) !== null).toBe(true));
    const editor = () => EditorView.findFromDOM(document.querySelector(".http-code-box .cm-editor") as HTMLElement)!;
    act(() => editor().dispatch({ changes: { from: 0, insert: "typed();" }, userEvent: "input.type" }));
    fireEvent.click(screen.getByRole("button", { name: "Move into the request…" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Move and turn on" })));
    await waitFor(() => expect(called("http_draft_apply").length).toBe(1));
    // The edit went first: written after, it would take the draft's place.
    expect(calls.map((c) => c.command).filter((c) => c === "http_script_save" || c === "http_draft_apply")).toEqual(["http_script_save", "http_draft_apply"]);
    await waitFor(() => expect(screen.queryByText("On — runs on Send") !== null).toBe(true));
    expect(editor().state.doc.toString()).toBe(DRAFT.code);
    // Left and come back to, still the draft: not what was typed before it.
    fireEvent.click(screen.getByRole("tab", { name: /Body/ }));
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    expect(editor().state.doc.toString()).toBe(DRAFT.code);
  });

  test("the dialog moves what it shows: a draft the chat writes again meanwhile is said, and the one read is refused", async () => {
    states = [{ item: "r1", listen: "test", codes: ["old();"], hashes: ["h2"], on: false }];
    drafts = [DRAFT];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Draft from “Check orders”" }) !== null).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Move into the request…" }));
    drafts = [{ ...DRAFT, code: "sneaky();" }];
    await act(async () => told.forEach((tell) => tell()));
    await waitFor(() => expect(screen.queryByText("sneaky();", { selector: "pre" }) !== null).toBe(true));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/The chat has written this draft again since you opened this/) !== null).toBe(true);
    expect((dialog.querySelector(".diff-view")?.textContent ?? "").includes("sneaky")).toBe(false);
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Move and turn on" })));
    await waitFor(() => expect(called("http_draft_apply").length).toBe(1));
    expect(called("http_draft_apply")).toEqual([{ draft: { collection: "c", request: "r1", listen: "test", chat: "chat-1", code: DRAFT.code } }]);
    await waitFor(() => expect(screen.queryByText(/The draft was not moved into the request/) !== null).toBe(true));
    expect(states.find((s) => s.item === "r1")?.on).toBe(false);
  });

  test("a draft whose code is a script off elsewhere says moving turns that on too, and code that does not show is said", async () => {
    const code = "pm.test('ok');";
    states = [{ item: null, listen: "test", codes: [code], hashes: ["h0"], on: false }];
    drafts = [{ ...DRAFT, code }, { ...DRAFT, chat: "chat-2", chatTitle: "Hidden", code: "x();\u202Ey();" }];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Draft from “Check orders”" }) !== null).toBe(true));
    const same = within(screen.getByRole("region", { name: "Draft from “Check orders”" }));
    expect(same.getByText("The same code is a script that is off: moving this turns it on too — Orders API · post-response") !== null).toBe(true);
    expect(same.queryByText(/characters that do not show/) === null).toBe(true);
    const hidden = within(screen.getByRole("region", { name: "Draft from “Hidden”" }));
    expect(hidden.getByText(/characters that do not show/) !== null).toBe(true);
    expect(hidden.queryByText(/The same code is a script/) === null).toBe(true);
  });

  test("a draft rejected goes and leaves the script; one the model wrote again since is not moved", async () => {
    states = [{ item: "r1", listen: "test", codes: ["old();"], hashes: ["h2"], on: false }];
    // A request of the same id in another collection: its draft is not this one's.
    drafts = [DRAFT, { ...DRAFT, chat: "chat-2", chatTitle: "Other", code: "other();" }, { ...DRAFT, collection: "c9", chat: "chat-3", chatTitle: "Elsewhere" }];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Draft from “Other”" }) !== null).toBe(true));
    await act(async () => fireEvent.click(within(screen.getByRole("region", { name: "Draft from “Other”" })).getByRole("button", { name: "Reject" })));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Draft from “Other”" }) === null).toBe(true));
    expect(called("http_draft_reject")).toEqual([{ draft: { collection: "c", request: "r1", listen: "test", chat: "chat-2", code: "other();" } }]);
    expect(screen.queryByRole("region", { name: "Draft from “Elsewhere”" }) === null).toBe(true);
    expect(document.querySelector(".http-code-box .cm-content")?.textContent).toBe("old();");

    // The model wrote again meanwhile: the move is refused, and says so.
    drafts = [{ ...DRAFT, code: "newer();" }];
    const block = within(screen.getByRole("region", { name: "Draft from “Check orders”" }));
    fireEvent.click(block.getByRole("button", { name: "Move into the request…" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Move and turn on" })));
    await waitFor(() => expect(screen.queryByText(/The draft was not moved into the request: The draft changed or is gone/) !== null).toBe(true));
    expect(document.querySelector(".http-code-box .cm-content")?.textContent).toBe("old();");
    // Read again: the newer one is what is shown now.
    await waitFor(() => expect(screen.queryByText("newer();", { selector: "pre" }) !== null).toBe(true));
  });

  test("a level with one script on and another off is marked off, in whichever order they come", async () => {
    for (const order of [
      [true, false],
      [false, true],
    ]) {
      states = [
        { item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: order[0] },
        { item: "r1", listen: "test", codes: ["x();"], hashes: ["h2"], on: order[1] },
      ];
      // The tree's open folders are kept: each round starts with it closed.
      localStorage.clear();
      await shown("r1");
      fireEvent.click(screen.getByText("Orders API"));
      await waitFor(() => expect(screen.queryByLabelText("Postman scripts, off") !== null).toBe(true));
      expect(screen.queryByLabelText("Postman scripts, on") === null).toBe(true);
      expect(screen.queryByLabelText("Scripts off") !== null).toBe(true);
      cleanup();
    }
  });

  test("turning all on shows each script's code first — the backend's, not the file as the page read it — and turns on only those", async () => {
    states = [
      { item: null, listen: "test", codes: ["kept();"], hashes: ["h0"], on: true },
      { item: "r1", listen: "prerequest", codes: ["first();", "runs();"], hashes: ["h1", "h2"], on: false },
      // Another request's: all of the collection's are turned on, not this request's alone.
      { item: "r2", listen: "test", codes: ["health();"], hashes: ["h3"], on: false },
    ];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    fireEvent.click(screen.getByRole("button", { name: "Turn on all 2 scripts off…" }));
    expect(screen.getByText("health();", { selector: "pre" }) !== null).toBe(true);
    expect(screen.getByText("request “Create order” · pre-request") !== null).toBe(true);
    expect(screen.getByText("first(); runs();", { selector: "pre", normalizer: (t) => t.replace(/\s+/g, " ").trim() }) !== null).toBe(true);
    expect(screen.queryByText(/pm\.environment\.set/, { selector: "pre" }) === null).toBe(true);
    expect(screen.queryByText("kept();", { selector: "pre" }) === null).toBe(true);
    expect(called("http_scripts_set")).toEqual([]);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Turn on 2 scripts" })));
    await waitFor(() => expect(called("http_scripts_set")).toEqual([{ collection: "c", hashes: ["h1", "h2", "h3"], on: true }]));
    await waitFor(() => expect(screen.queryByRole("button", { name: /Turn on all/ }) === null).toBe(true));
  });

  test("a request's own script is typed on its tab and written a moment after; the one above it is read only", async () => {
    states = [{ item: null, listen: "prerequest", codes: ["collection();"], hashes: ["h0"], on: false }];
    await shown("r2");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    // Only the request's own is edited here: a note about edits is not for the collection's.
    expect(screen.queryByText("An edit leaves it off until you turn it on.") === null).toBe(true);
    const segments = within(screen.getByRole("tablist", { name: "Scripts, in the order they run" }));
    expect(segments.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Orders API · pre-request", "Pre-request", "Post-response"]);
    const editor = () => EditorView.findFromDOM(document.querySelector(".http-code-box .cm-editor") as HTMLElement)!;
    expect(editor().state.readOnly).toBe(true);
    fireEvent.click(segments.getByRole("tab", { name: "Post-response" }));
    expect(screen.getByText("Empty — what you write here runs once you turn it on.") !== null).toBe(true);
    expect(editor().state.readOnly).toBe(false);
    act(() => editor().dispatch({ changes: { from: 0, insert: "pm.test('ok');" }, userEvent: "input.type" }));
    act(() => editor().dispatch({ changes: { from: 14, insert: " // done" }, userEvent: "input.type" }));
    expect(called("http_script_save")).toEqual([]);
    await waitFor(() => expect(called("http_script_save")).toEqual([{ collection: "c", requestId: "r2", listen: "test", code: "pm.test('ok'); // done" }]));
    // Read again, as the backend says: written where there was none, it waits for the switch.
    await waitFor(() => expect(screen.queryByText("Off — does not run") !== null).toBe(true));
    expect(editor().state.doc.toString()).toBe("pm.test('ok'); // done");
    // Left and come back to, the tab shows what was typed, not what was read before.
    fireEvent.click(screen.getByRole("tab", { name: /Body/ }));
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    fireEvent.click(within(screen.getByRole("tablist", { name: "Scripts, in the order they run" })).getByRole("tab", { name: /Post-response/ }));
    expect(editor().state.doc.toString()).toBe("pm.test('ok'); // done");
  });

  test("what is typed while a save is out stays: the save's state read back does not take it away", async () => {
    scriptSaves = [];
    await shown("r2");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    fireEvent.click(screen.getByRole("tab", { name: "Pre-request" }));
    const editor = () => EditorView.findFromDOM(document.querySelector(".http-code-box .cm-editor") as HTMLElement)!;
    act(() => editor().dispatch({ changes: { from: 0, insert: "a" }, userEvent: "input.type" }));
    await waitFor(() => expect(scriptSaves!.length).toBe(1));
    act(() => editor().dispatch({ changes: { from: 1, insert: "b" }, userEvent: "input.type" }));
    const reads = called("http_scripts").length;
    // The first save lands, and what it wrote — "a" — is read back while "ab" is typed.
    states = [{ item: "r2", listen: "prerequest", codes: ["a"], hashes: ["ha"], on: true }];
    await act(async () => scriptSaves![0]());
    await waitFor(() => expect(called("http_scripts").length).toBe(reads + 1));
    await waitFor(() => expect(screen.queryByText("On — runs on Send") !== null).toBe(true));
    expect(editor().state.doc.toString()).toBe("ab");
    scriptSaves = null;
  });

  test("a script typed and not yet written goes before the send; an edit of one off says it stays off", async () => {
    states = [{ item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: false }];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    expect(screen.getByText("An edit leaves it off until you turn it on.") !== null).toBe(true);
    const editor = EditorView.findFromDOM(document.querySelector(".http-code-box .cm-editor") as HTMLElement)!;
    act(() => editor.dispatch({ changes: { from: 0, insert: "y();" }, userEvent: "input.type" }));
    await act(async () => fireEvent.click(sendButton()));
    await waitFor(() => expect(called("http_send").length).toBe(1));
    const order = calls.map((c) => c.command).filter((c) => c === "http_script_save" || c === "http_send");
    expect(order).toEqual(["http_script_save", "http_send"]);
    expect(called("http_script_save")).toEqual([{ collection: "c", requestId: "r1", listen: "prerequest", code: "y();x();" }]);
  });

  test("a switch goes after what was typed: the save first, then the switch, then one read", async () => {
    states = [{ item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: true }];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    const editor = EditorView.findFromDOM(document.querySelector(".http-code-box .cm-editor") as HTMLElement)!;
    act(() => editor.dispatch({ changes: { from: 0, insert: "y();" }, userEvent: "input.type" }));
    const before = calls.length;
    await act(async () => fireEvent.click(screen.getByRole("switch", { name: "Run Pre-request" })));
    const order = () => calls.slice(before).map((c) => c.command).filter((c) => c.startsWith("http_script"));
    await waitFor(() => expect(order()).toEqual(["http_script_save", "http_scripts", "http_scripts_set", "http_scripts"]));
  });

  test("a switch refused says the scripts changed", async () => {
    states = [{ item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["stale"], on: false }];
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Scripts/ }));
    await act(async () => fireEvent.click(screen.getByRole("switch", { name: "Run Pre-request" })));
    expect((await screen.findByText(/changed — look at them again/)) !== null).toBe(true);
    expect(screen.getByText("Off — does not run") !== null).toBe(true);
  });

  test("scripts that did not run say how many, and the note's button opens the Scripts tab", async () => {
    answer = reported({ off: ['pre-request of request "Create order"'] });
    await shown("r1");
    await act(async () => fireEvent.click(sendButton()));
    expect((await screen.findByText(/1 script did not run: it is off/)) !== null).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Show scripts" }));
    expect(screen.getByRole("tab", { name: /Scripts/ }).getAttribute("aria-selected")).toBe("true");
  });

  test("the Tests tab counts what failed and shows each test, what stopped the scripts and the console", async () => {
    answer = reported({
      tests: [
        { name: "is ok", passed: true, skipped: false },
        { name: "has id", passed: false, skipped: false, error: "expected undefined to exist" },
        { name: "later", passed: false, skipped: true },
      ],
      console: [{ level: "warn", text: "slow" }],
      error: "Script ran out of time",
    });
    await shown("r2");
    await act(async () => fireEvent.click(sendButton()));
    const tab = await screen.findByRole("tab", { name: /Tests/ });
    expect(tab.textContent).toContain("2 failed");
    fireEvent.click(tab);
    const row = (name: string) => screen.getByText(name).closest("li")?.className ?? "";
    expect(row("is ok")).toContain("http-test-passed");
    expect(row("has id")).toContain("http-test-failed");
    expect(row("later")).toContain("http-test-skipped");
    expect(screen.getByText("expected undefined to exist") !== null).toBe(true);
    expect(screen.getByText("Script ran out of time") !== null).toBe(true);
    expect(screen.getByText("slow").className).toContain("http-console-warn");
  });

  test("tests that passed are counted; a request with no scripts says so", async () => {
    answer = reported({ tests: [{ name: "is ok", passed: true, skipped: false }] });
    await shown("r2");
    await act(async () => fireEvent.click(sendButton()));
    await waitFor(() => expect(screen.getByRole("tab", { name: /Tests/ }).textContent).toBe("Tests1"));
    answer = reported({});
    await act(async () => fireEvent.click(sendButton()));
    await waitFor(() => expect(screen.getByRole("tab", { name: /Tests/ }).textContent).toBe("Tests"));
    fireEvent.click(screen.getByRole("tab", { name: /Tests/ }));
    expect(screen.getByText("This request has no scripts.") !== null).toBe(true);
  });

  test("while a request with scripts is out the page says a script may be waiting; one without says nothing", async () => {
    holdSends = true;
    states = [{ item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: true }];
    await shown("r1");
    await waitFor(() => expect(screen.queryByLabelText("Scripts on") !== null).toBe(true));
    await act(async () => fireEvent.click(sendButton()));
    expect((await screen.findByText("Scripts may be waiting…")) !== null).toBe(true);
    fireEvent.click(screen.getByText("Orders API"));
    fireEvent.click(screen.getByText("Health"));
    await act(async () => fireEvent.click(sendButton()));
    expect(screen.queryByText("Scripts may be waiting…") === null).toBe(true);
  });

  test("a deleted request keeps its history, with nothing to open", async () => {
    await shown();
    const openFromHistory = async () => {
      fireEvent.click(screen.getByRole("button", { name: "History" }));
      fireEvent.click(await screen.findByTitle(/^GET \{\{baseUrl\}\}\/health/));
      return screen.queryByRole("button", { name: "Open request" });
    };
    expect(await openFromHistory()).not.toBeNull();
    fireEvent.click(screen.getByLabelText("Close panel"));
    fireEvent.click(screen.getByText("Orders API"));
    fireEvent.click(screen.getByLabelText("Actions for Health"));
    fireEvent.click(screen.getByRole("option", { name: /Delete/ }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await openFromHistory()).toBeNull();
  });

  test("an environment is off to the agent until its form says otherwise", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    const access = screen.getByTitle("What the agent may do here");
    expect(access.textContent).toContain("Off");
    fireEvent.click(access);
    fireEvent.click(screen.getByRole("option", { name: /Read only/ }));
    expect(screen.getByTitle("What the agent may do here").textContent).toContain("Read only");
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).agent).toBe("readOnly");
  });

  test("the writing modes are offered and saved by name", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.click(screen.getByTitle("What the agent may do here"));
    expect(screen.getByRole("option", { name: /Writes as other tools/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("option", { name: /Ask for writes/ }));
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).agent).toBe("askForWrites");
  });

  test("a renamed secret goes back as the mask, which the backend refuses rather than saving it empty", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.change(screen.getAllByLabelText("Variable")[1], { target: { value: "apiToken" } });
    fireEvent.focusOut(screen.getAllByLabelText("Variable")[1]);
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    const saved = called("http_environment_save")[0]!.environment as Environment;
    expect(saved.variables[1]).toEqual({ key: "apiToken", value: "•••", secret: true, enabled: true });
  });

  test("a saved secret is never in the form: left empty it is kept, and a bad certificate is said", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    const values = screen.getAllByLabelText("Value") as HTMLInputElement[];
    expect(values[1].value).toBe("");
    expect(values[1].placeholder).toContain("saved — type to replace");
    expect(screen.queryByRole("button", { name: "Save" }) === null).toBe(true);
    // A field left with nothing changed saves nothing.
    fireEvent.focusOut(values[1]);
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_environment_save")).toEqual([]);
    fireEvent.change(values[0], { target: { value: "https://stage2.test" } });
    fireEvent.focusOut(values[0]);
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    const saved = called("http_environment_save")[0]!.environment as Environment;
    expect(saved.variables).toEqual([
      { key: "baseUrl", value: "https://stage2.test", secret: false, enabled: true },
      { key: "token", value: "•••", secret: true, enabled: true },
    ]);

    fireEvent.change(screen.getByPlaceholderText(/PEM/), { target: { value: "junk" } });
    fireEvent.focusOut(screen.getByPlaceholderText(/PEM/));
    expect(await screen.findByText("the certificate: no certificate in it")).toBeDefined();
    // Refused, it is tried again as the field is next left, though nothing changed since.
    fireEvent.focusOut(screen.getByPlaceholderText(/PEM/));
    await waitFor(() => expect(called("http_environment_save").length).toBe(3));
  });

  test("a shown secret edited is saved as typed and stays shown", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.click(screen.getByLabelText("Show token"));
    const value = () => (screen.getAllByLabelText("Value") as HTMLInputElement[])[1];
    await waitFor(() => expect(value().value).toBe("hunter2"));
    fireEvent.change(value(), { target: { value: "hunter3" } });
    fireEvent.focusOut(value());
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).variables[1].value).toBe("hunter3");
    await new Promise((r) => setTimeout(r, 20));
    expect([value().value, value().type]).toEqual(["hunter3", "text"]);
  });

  test("what a script writes while the form is open is kept as the form saves; what was typed stays", async () => {
    stage.variables.push({ key: "orderId", value: "1", secret: false, enabled: true });
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    const value = (i: number) => (screen.getAllByLabelText("Value") as HTMLInputElement[])[i];
    const var_ = (key: string, value: string) => ({ key, value, secret: false, enabled: true });
    // An agent's send ran a script that set a variable: the form shows it, and has nothing to save.
    stage = { ...stage, variables: [STAGE.variables[0], STAGE.variables[1], var_("orderId", "7")] };
    act(() => told.forEach((tell) => tell()));
    await waitFor(() => expect(value(2).value).toBe("7"));
    fireEvent.focusOut(value(2));
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_environment_save")).toEqual([]);

    fireEvent.change(value(0), { target: { value: "https://mine.test" } });
    // Another, while a field is being typed in: it sets the one typed in and writes one more.
    stage = { ...stage, variables: [var_("baseUrl", "https://script.test"), STAGE.variables[1], var_("orderId", "7"), var_("session", "s1")] };
    act(() => told.forEach((tell) => tell()));
    await waitFor(() => expect(screen.getAllByLabelText("Value").length).toBe(4));
    expect([value(0).value, value(2).value, value(3).value]).toEqual(["https://mine.test", "7", "s1"]);
    fireEvent.focusOut(value(0));
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).variables).toEqual([
      var_("baseUrl", "https://mine.test"),
      { key: "token", value: "•••", secret: true, enabled: true },
      var_("orderId", "7"),
      var_("session", "s1"),
    ]);
    // Read back as saved, nothing is left to save.
    await new Promise((r) => setTimeout(r, 20));
    fireEvent.focusOut(value(2));
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_environment_save").length).toBe(1);
  });

  test("a variable turned off is saved at once", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.click(screen.getByLabelText("Use baseUrl"));
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).variables[0].enabled).toBe(false);
  });

  test("what is typed is saved as the form closes, with no field left", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.change(screen.getAllByLabelText("Value")[0], { target: { value: "https://stage3.test" } });
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).variables[0].value).toBe("https://stage3.test");
  });

  test("a saved secret is read only by its eye, shown until hidden, and sealed again once typed over", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    expect(called("http_environment_secret").length).toBe(0);
    expect(screen.queryByLabelText("Show baseUrl") === null).toBe(true);
    fireEvent.click(screen.getByLabelText("Show token"));
    const value = () => (screen.getAllByLabelText("Value") as HTMLInputElement[])[1];
    await waitFor(() => expect(value().value).toBe("hunter2"));
    expect(called("http_environment_secret")).toEqual([{ id: "stage", key: "token" }]);
    expect(value().type).toBe("text");
    fireEvent.click(screen.getByLabelText("Hide token"));
    expect(value().type).toBe("password");
    expect(value().value).toBe("hunter2");
    fireEvent.click(screen.getByLabelText("Show token"));
    expect(value().type).toBe("text");
    expect(called("http_environment_secret").length).toBe(1);
    // Only looked at, it is no change: leaving the field saves nothing, and it stays shown.
    fireEvent.focusOut(value());
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_environment_save")).toEqual([]);
    expect(value().value).toBe("hunter2");
    // Another field saved: the one the eye shows stays shown.
    fireEvent.change(screen.getAllByLabelText("Value")[0], { target: { value: "https://stage4.test" } });
    fireEvent.focusOut(screen.getAllByLabelText("Value")[0]);
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    await new Promise((r) => setTimeout(r, 20));
    expect([value().value, value().type]).toEqual(["hunter2", "text"]);
    // The lock off and on again does not show it without the eye; each is saved at once,
    // and the value it had goes as the mask, never as itself.
    fireEvent.click(screen.getByLabelText("token is a secret"));
    fireEvent.click(screen.getByLabelText("Make token a secret"));
    expect(value().type).toBe("password");
    const sentToken = () => called("http_environment_save").slice(1).map((c) => (c!.environment as Environment).variables[1].value);
    await waitFor(() => expect(sentToken()).toEqual(["•••", "•••"]));
    // A value typed over it goes as itself, and the field is sealed again.
    fireEvent.change(value(), { target: { value: "new-one" } });
    fireEvent.focusOut(value());
    await waitFor(() => expect(value().value).toBe(""));
    expect(sentToken()).toEqual(["•••", "•••", "new-one"]);
    expect(value().type).toBe("password");
  });

  test("a secret read while a row above is removed lands in its own row, not the one that moved up", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.click(screen.getByRole("button", { name: "Variable" }));
    fireEvent.click(screen.getByLabelText("Show token"));
    fireEvent.click(screen.getByLabelText("Remove baseUrl"));
    const values = () => (screen.getAllByLabelText("Value") as HTMLInputElement[]).map((v) => v.value);
    await waitFor(() => expect(values()).toEqual(["hunter2", ""]));
  });

  test("a renamed secret has no eye: nothing is saved under its new name", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.change(screen.getAllByLabelText("Variable")[1], { target: { value: "apiToken" } });
    expect(screen.queryByLabelText("Show apiToken") === null).toBe(true);
  });

  test("a {{ in a field lists the variables in scope, a secret masked, and a pick closes it", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown("r2");
    const address = screen.getByLabelText("Address") as HTMLInputElement;
    fireEvent.change(address, { target: { value: "{{" } });
    const options = () => screen.queryAllByRole("option").map((o) => o.textContent);
    expect(options()).toEqual(["{{orderId}}9", "{{baseUrl}}https://stage.test", "{{token}}•••"]);
    fireEvent.change(address, { target: { value: "{{to" } });
    expect(options()).toEqual(["{{token}}•••"]);
    fireEvent.keyDown(address, { key: "Escape", code: "Escape" });
    expect(options()).toEqual([]);

    fireEvent.change(address, { target: { value: "{{" } });
    fireEvent.keyDown(address, { key: "ArrowDown", code: "ArrowDown" });
    expect(screen.getByRole("option", { selected: true }).textContent).toContain("{{baseUrl}}");
    fireEvent.keyDown(address, { key: "ArrowUp", code: "ArrowUp" });
    fireEvent.keyDown(address, { key: "ArrowUp", code: "ArrowUp" });
    expect(screen.getByRole("option", { selected: true }).textContent).toContain("{{token}}");
    fireEvent.keyDown(address, { key: "ArrowDown", code: "ArrowDown" });
    fireEvent.keyDown(address, { key: "ArrowDown", code: "ArrowDown" });
    fireEvent.keyDown(address, { key: "Enter", code: "Enter" });
    expect(address.value).toBe("{{baseUrl}}");
    expect(address.selectionStart).toBe(11);
    expect(options()).toEqual([]);

    fireEvent.change(address, { target: { value: "{{baseUrl}}/x?t={{" } });
    fireEvent.pointerDown(screen.getByRole("option", { name: /token/ }));
    expect(address.value).toBe("{{baseUrl}}/x?t={{token}}");

    // Typed in the middle: the caret lands after the variable, not at the end.
    fireEvent.change(address, { target: { value: "{{/x" } });
    // A click there: React makes onSelect from the mouseup, not from a `select` event.
    act(() => address.focus());
    address.setSelectionRange(2, 2);
    fireEvent.mouseUp(address);
    fireEvent.keyDown(address, { key: "Tab", code: "Tab" });
    expect(address.value).toBe("{{orderId}}/x");
    expect(address.selectionStart).toBe(11);

    fireEvent.change(address, { target: { value: "{{" } });
    fireEvent.blur(address);
    expect(options()).toEqual([]);
  });

  test("a {{ in the body lists the same variables, and the registry's key puts one in", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown("r1");
    fireEvent.click(screen.getByRole("tab", { name: /Body/ }));
    fireEvent.click(screen.getByRole("radio", { name: "raw" }));
    const view = EditorView.findFromDOM(document.querySelector(".http-editor .cm-editor") as HTMLElement)!;
    act(() => view.dispatch({ changes: { from: 0, insert: '{"t": "{{to}}"' }, selection: { anchor: 11 }, userEvent: "input.type" }));
    await waitFor(() => expect(currentCompletions(view.state).map((c) => [c.label, c.detail])).toEqual([["{{token}}", "•••"]]));
    // CodeMirror takes no pick for its `interactionDelay` (75 ms) after the list opens.
    await new Promise((done) => setTimeout(done, 100));
    act(() => {
      view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true }));
    });
    expect(view.state.doc.toString()).toBe('{"t": "{{token}}"');
    expect(view.state.selection.main.head).toBe(16);
  });

  test("a variable missing from the auth marks the Auth tab, not the address", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown("r4");
    // Booleans, not elements: a failing expect prints an element's whole page, for minutes.
    expect(document.querySelector(".http-url-row.missing") !== null).toBe(false);
    const tab = screen.getByRole("tab", { name: /Auth/ });
    expect(tab.querySelector('[aria-label="Not in Stage: {{apiToken}}"]') !== null).toBe(true);
    // The request's Headers, before the response's.
    expect(screen.getAllByRole("tab", { name: /Headers/ })[0].querySelector(".http-dot-amber") !== null).toBe(false);
  });

  test("a curl pasted into the address is read as the request, not typed in", async () => {
    await shown("r2");
    const address = screen.getByLabelText("Address") as HTMLInputElement;
    const command = "curl 'https://api.test/v1/items' -H 'accept: application/json'";
    const notTyped = fireEvent.paste(address, { clipboardData: { getData: (type: string) => (type === "text/plain" || type === "text" ? command : "") } });
    expect(notTyped).toBe(false);
    await waitFor(() => expect(called("http_parse_curl")).toEqual([{ command }]));
  });

  test("a curl pasted where the paste event carries no text is still read, once it is in the address", async () => {
    await shown("r2");
    const address = screen.getByLabelText("Address") as HTMLInputElement;
    const command = "curl 'https://api.test/v1/items'";
    fireEvent.paste(address, { clipboardData: { getData: () => "" } });
    fireEvent.input(address, { target: { value: command }, inputType: "insertFromPaste" });
    await waitFor(() => expect(called("http_parse_curl")).toEqual([{ command }]));
    // Typed, it is an address like any other.
    fireEvent.input(address, { target: { value: "curl x" }, inputType: "insertText" });
    await new Promise((r) => setTimeout(r, 20));
    expect(called("http_parse_curl").length).toBe(1);
  });

  test("a request with scripts is marked; one with a body kept from Postman shows it and is refused", async () => {
    states = [{ item: "r1", listen: "prerequest", codes: ["x();"], hashes: ["h1"], on: true }];
    await shown("r3");
    fireEvent.click(screen.getByText("Orders API"));
    await waitFor(() => expect(screen.queryAllByLabelText("Postman scripts, on").length).toBe(1));
    expect(screen.queryByLabelText("Postman scripts, off") === null).toBe(true);
    fireEvent.click(screen.getByRole("tab", { name: /Body/ }));
    expect(screen.getByText(/kept from Postman as it came/)).toBeDefined();
    await act(async () => fireEvent.click(sendButton()));
    expect(await screen.findByText("a graphql body is kept from Postman and not sent yet")).toBeDefined();
  });
});
