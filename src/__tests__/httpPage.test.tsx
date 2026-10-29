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
    { id: "r3", name: "Upload", request: { method: "POST", url: "{{baseUrl}}/files", body: { mode: "formdata", formdata: [] } } },
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

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "http_collection_list") return Promise.resolve([{ id: "c", file: structuredClone(FILE), error: null }]);
    if (command === "http_environment_list") return Promise.resolve({ environments: [structuredClone(STAGE)], globals: [] });
    if (command === "http_history_list") return Promise.resolve([ENTRY]);
    if (command === "http_collection_new") return Promise.resolve({ id: "c2", file: { info: { name: args!.name }, item: [] }, error: null });
    if (command === "http_send") {
      if (args!.requestId === "r3") return Promise.reject("a formdata body is kept from Postman and not sent yet");
      return holdSends ? new Promise<HttpResponseView>(() => {}) : Promise.resolve(null);
    }
    if (command === "http_environment_save") {
      const env = args!.environment as Environment;
      return env.trustedCert === "junk" ? Promise.reject("the certificate: no certificate in it") : Promise.resolve(null);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
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
  holdSends = false;
});

describe("the HTTP page", () => {
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
    expect(called("http_send")).toEqual([{ collection: "c", requestId: "r2", request: "{{baseUrl}}/health", environment: null }]);
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
    expect(await screen.findByText("a formdata body is kept from Postman and not sent yet")).toBeDefined();
  });
});
