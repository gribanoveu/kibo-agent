import { afterAll, beforeEach, describe, expect, mock, spyOn, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { HttpHistoryEntry, HttpResponseView } from "../lib/httpCommands";
import type { Environment } from "../lib/httpClient";

// The HTTP page on its stores: a collection, its requests and an environment
// with a secret the page has only ever seen masked.

const FILE = {
  info: { name: "Orders API", schema: "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
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
let holdSends = false;
let trust: { hash: string | null; count: number; trusted: boolean } = { hash: null, count: 0, trusted: false };
let jar: { domain: string; path: string; name: string; expiresAt?: number | null }[] = [];
let answer: HttpResponseView;
const reported = (scripts: Partial<HttpResponseView["scripts"]>): HttpResponseView => ({
  at: 1,
  environment: null,
  outcome: { kind: "response", status: 200, headers: [], url: "http://x", elapsedMs: 5 },
  body: "ok",
  size: 2,
  scripts: { notRun: false, tests: [], console: [], changed: false, ...scripts },
});

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "http_collection_list") return Promise.resolve([{ id: "c", file: structuredClone(FILE), error: null }]);
    if (command === "http_environment_list") return Promise.resolve({ environments: [structuredClone(STAGE)], globals: [] });
    if (command === "http_history_list") return Promise.resolve([ENTRY]);
    if (command === "http_script_trust") return Promise.resolve(trust);
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
      return env.trustedCert === "junk" ? Promise.reject("the certificate: no certificate in it") : Promise.resolve(null);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

mock.module("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

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
  holdSends = false;
  trust = { hash: null, count: 0, trusted: false };
  jar = [];
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

  test("the first send of scripts not trusted asks, naming the collection, what they can reach and how many", async () => {
    trust = { hash: "h1", count: 3, trusted: false };
    await shown("r2");
    await act(async () => fireEvent.click(sendButton()));
    expect((await screen.findByText(/Run the scripts of “Orders API”/)) !== null).toBe(true);
    expect(screen.getByText(/3 scripts\. A script reads every variable, secrets too/) !== null).toBe(true);
    fireEvent.keyDown(window, { code: "Enter", key: "Enter", ...MOD });
    expect(called("http_script_trust").length).toBe(1);
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Run scripts" })));
    await waitFor(() => expect(called("http_script_trust_set")).toEqual([{ collection: "c", hash: "h1" }]));
  });

  test("the question can be left: Cancel sends nothing, the other button sends without them", async () => {
    trust = { hash: "h1", count: 1, trusted: false };
    await shown("r2");
    await act(async () => fireEvent.click(sendButton()));
    await act(async () => fireEvent.click(await screen.findByRole("button", { name: "Cancel" })));
    expect(called("http_send")).toEqual([]);
    await act(async () => fireEvent.click(sendButton()));
    await act(async () => fireEvent.click(await screen.findByRole("button", { name: "Send without scripts" })));
    await waitFor(() => expect(called("http_send").length).toBe(1));
    expect(called("http_script_trust_set")).toEqual([]);
  });

  test("scripts that did not run say so, and the note's button asks again", async () => {
    answer = reported({ notRun: true });
    await shown("r2");
    await act(async () => fireEvent.click(sendButton()));
    expect((await screen.findByText(/The collection's scripts did not run/)) !== null).toBe(true);
    trust = { hash: "h1", count: 1, trusted: false };
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Send with scripts…" })));
    expect((await screen.findByText(/Run the scripts of/)) !== null).toBe(true);
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
    await shown("r1");
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
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    expect((called("http_environment_save")[0]!.environment as Environment).agent).toBe("askForWrites");
  });

  test("a renamed secret goes back as the mask, which the backend refuses rather than saving it empty", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.change(screen.getAllByLabelText("Variable")[1], { target: { value: "apiToken" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
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
    fireEvent.change(values[0], { target: { value: "https://stage2.test" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(called("http_environment_save").length).toBe(1));
    const saved = called("http_environment_save")[0]!.environment as Environment;
    expect(saved.variables).toEqual([
      { key: "baseUrl", value: "https://stage2.test", secret: false, enabled: true },
      { key: "token", value: "•••", secret: true, enabled: true },
    ]);

    fireEvent.change(screen.getByPlaceholderText(/PEM/), { target: { value: "junk" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("the certificate: no certificate in it")).toBeDefined();
  });

  test("a saved secret is read only by its eye, shown until hidden, and sealed again once saved", async () => {
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
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(value().value).toBe(""));
    expect(value().type).toBe("password");
  });

  test("a renamed secret has no eye: nothing is saved under its new name", async () => {
    localStorage.setItem("http.environment", JSON.stringify("stage"));
    await shown();
    fireEvent.click(screen.getByLabelText("Edit the environment"));
    fireEvent.change(screen.getAllByLabelText("Variable")[1], { target: { value: "apiToken" } });
    expect(screen.queryByLabelText("Show apiToken") === null).toBe(true);
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
    await shown("r3");
    fireEvent.click(screen.getByText("Orders API"));
    expect(screen.getAllByLabelText("Has Postman scripts").length).toBe(1);
    fireEvent.click(screen.getByRole("tab", { name: /Body/ }));
    expect(screen.getByText(/kept from Postman as it came/)).toBeDefined();
    await act(async () => fireEvent.click(sendButton()));
    expect(await screen.findByText("a graphql body is kept from Postman and not sent yet")).toBeDefined();
  });
});
