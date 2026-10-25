import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { KnowledgeBase, KnowledgeDocument } from "../lib/knowledge";

// The Knowledge page (docs/40-knowledge-base.md, decision 9): the domains as
// folders, in one its repositories' documents as cards by kind; a document
// opens on the right with its evidence, and the page's changes answer with
// the base after them.

let base: KnowledgeBase;
let calls: { command: string; args?: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "knowledge_base") return Promise.resolve(structuredClone(base));
    if (command === "knowledge_document")
      return Promise.resolve(`---\nid: ${args!.id}\nevidence:\n- src/refunds.rs#L1-L9\nconfidence: confirmed\n---\n# Body of ${args!.id}\n`);
    if (command === "knowledge_move") {
      base.documents = base.documents.map((d) => (d.id === args!.id ? { ...d, domain: args!.domain as string } : d));
      return Promise.resolve(structuredClone(base));
    }
    if (command === "knowledge_delete") {
      base.documents = base.documents.filter((d) => d.id !== args!.id);
      return Promise.resolve(structuredClone(base));
    }
    return Promise.resolve(structuredClone(base));
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Map<string, Set<(message: { payload: unknown }) => void>>();
const emit = (channel: string, payload: unknown) => act(() => listeners.get(channel)?.forEach((handler) => handler({ payload })));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (!listeners.has(channel)) listeners.set(channel, new Set());
    listeners.get(channel)!.add(handler);
    return Promise.resolve(() => listeners.get(channel)!.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { KnowledgePage } = await import("../components/KnowledgePage");
const { splitDocument } = await import("../lib/knowledge");

const doc = (id: string, domain: string, kind: KnowledgeDocument["kind"], title: string, more: Partial<KnowledgeDocument> = {}): KnowledgeDocument => ({
  id,
  domain,
  repo: id.split("/")[0],
  kind,
  title,
  confidence: "inferred",
  stale: false,
  cited: true,
  written: 1_790_000_000,
  lead: `What ${title} does.`,
  ...more,
});

beforeEach(() => {
  calls = [];
  base = {
    domains: [
      { domain: { id: "payments", name: "Payments", description: "Charging and refunds" }, documents: 3, repos: 2 },
      { domain: { id: "general", name: "General", description: "Whatever fits no other" }, documents: 0, repos: 0 },
      { domain: { id: "auth", name: "Auth", description: "Sign-in" }, documents: 0, repos: 0 },
    ],
    repos: [
      { name: "billing", path: "/src/billing", commit: "abc", documents: 2, stale: 1, written: 1_790_000_000 },
      { name: "gateway", path: "/src/gateway", commit: null, documents: 1, stale: 0, written: null },
    ],
    documents: [
      doc("billing/refunds-api", "payments", "api", "Refunds API", { stale: true }),
      doc("billing/overview", "payments", "service", "Billing service", { confidence: "confirmed" }),
      doc("gateway/charge", "payments", "algorithm", "Charge retry", { cited: false }),
    ],
  };
});

const page = () =>
  render(<KnowledgePage openPane={() => {}} openPage={() => {}} openChat={() => {}} removeChat={() => Promise.resolve(true)} chats={[]} />);
const into = async (domain: string) => {
  fireEvent.click(await screen.findByRole("button", { name: new RegExp(`^${domain}`) }));
  return screen.getByRole("region", { name: domain });
};
const titles = (repo: string) =>
  screen
    .getAllByRole("listbox", { name: new RegExp(` of ${repo} in Payments$`) })
    .flatMap((list) => within(list).getAllByRole("option"))
    .map((o) => o.querySelector(".knowledge-title")?.textContent);

describe("KnowledgePage", () => {
  test("shows the domains as folders, and in one its repositories' documents by kind, with their marks", async () => {
    page();
    expect((await screen.findByRole("button", { name: /^Payments/ })).textContent).toContain("3 documents · 2 repositories");
    // An empty "general" is left out; another empty domain is shown, to be merged or described.
    expect(screen.queryByRole("button", { name: /^General/ })).toBeNull();
    expect(screen.getByRole("button", { name: /^Auth/ })).toBeTruthy();

    const payments = await into("Payments");
    expect(titles("billing")).toEqual(["Billing service", "Refunds API"]);
    expect(titles("gateway")).toEqual(["Charge retry"]);
    expect(payments.textContent).toContain("1 stale");
    expect(screen.getByRole("option", { name: /Refunds API/ }).textContent).toContain("What Refunds API does.");
    expect(screen.getByRole("option", { name: /Refunds API/ }).textContent).toContain("stale");
    expect(screen.getByRole("option", { name: /Charge retry/ }).textContent).toContain("no evidence");
    // Only what is off is marked: "confirmed" is the rule, "inferred" the exception.
    expect(screen.getByRole("option", { name: /Charge retry/ }).textContent).toContain("inferred");
    expect(screen.getByRole("option", { name: /Billing service/ }).textContent).not.toContain("confirmed");
    expect(screen.getByRole("listbox", { name: "API of billing in Payments" }).textContent).toContain("Refunds API");

    fireEvent.click(screen.getByRole("button", { name: "All domains" }));
    expect((await into("Auth")).textContent).toContain("No documents in it.");
  });

  test("opens a document with its frontmatter, moves it and deletes it", async () => {
    page();
    await into("Payments");
    fireEvent.click(screen.getByRole("option", { name: /Refunds API/ }));
    await waitFor(() => expect(screen.getByText("Body of billing/refunds-api")).toBeTruthy());
    // The text first; the frontmatter's lists closed over it.
    expect(screen.getByText("src/refunds.rs#L1-L9").closest("details")?.open).toBe(false);
    expect(calls).toContainEqual({ command: "knowledge_document", args: { id: "billing/refunds-api" } });

    fireEvent.click(screen.getByTitle("Move it to another domain"));
    fireEvent.click(screen.getByRole("option", { name: /^Auth/ }));
    await waitFor(() => expect(calls).toContainEqual({ command: "knowledge_move", args: { id: "billing/refunds-api", domain: "auth" } }));
    // Gone from the folder it left, still open where it went.
    await waitFor(() => expect(screen.queryByRole("option", { name: /Refunds API/ })).toBeNull());
    expect(screen.getByTitle("Move it to another domain").textContent).toContain("Auth");

    fireEvent.click(screen.getByRole("button", { name: "Delete document" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toContainEqual({ command: "knowledge_delete", args: { id: "billing/refunds-api" } }));
    await waitFor(() => expect(screen.queryByText("Body of billing/refunds-api")).toBeNull());
  });

  test("a document opens in a sheet over the page; Escape closes what is on top first", async () => {
    page();
    await into("Payments");
    fireEvent.click(screen.getByRole("option", { name: /Refunds API/ }));
    await waitFor(() => screen.getByText("Body of billing/refunds-api"));
    // The page stays as wide as it was, its cards still there to pick another.
    expect(screen.getByRole("complementary", { name: "Document billing/refunds-api" }).className).toBe("page-sheet");
    fireEvent.click(screen.getByRole("option", { name: /Billing service/ }));
    await waitFor(() => screen.getByText("Body of billing/overview"));

    fireEvent.click(screen.getByRole("button", { name: "Delete document" }));
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByText("Body of billing/overview")).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(screen.queryByRole("complementary", { name: /^Document/ })).toBeNull();
  });

  test("searches every domain's titles, leads, ids and kinds, keeping only the domains with a match", async () => {
    base.domains[2].documents = 1;
    base.documents.push(doc("billing/login", "auth", "howto", "Sign in", { lead: "Tokens expire after a retry window." }));
    page();
    await into("Payments");
    const search = screen.getByRole("searchbox", { name: "Search the knowledge base" });
    fireEvent.change(search, { target: { value: "retry" } });
    expect(screen.queryByRole("option", { name: /Refunds API/ })).toBeNull();
    expect(within(screen.getByRole("listbox", { name: "Found in Payments" })).getByRole("option", { name: /Charge retry/ })).toBeTruthy();
    // Another folder's document, found by its lead.
    expect(within(screen.getByRole("listbox", { name: "Found in Auth" })).getByRole("option", { name: /Sign in/ }).textContent).toContain("How-to");
    fireEvent.change(search, { target: { value: "refunds" } });
    expect(screen.queryByRole("region", { name: "Auth" })).toBeNull();
    fireEvent.change(search, { target: { value: "flows" } });
    expect(screen.getByRole("option", { name: /Charge retry/ })).toBeTruthy();
    fireEvent.change(search, { target: { value: "nothing like it" } });
    expect(screen.getByText("Nothing matches “nothing like it”.")).toBeTruthy();
  });

  test("merges a domain into another and deletes a repository, each after asking", async () => {
    page();
    await into("Payments");
    fireEvent.click(screen.getByTitle("Actions for Payments"));
    fireEvent.click(screen.getByRole("option", { name: /^Merge into Auth/ }));
    expect(calls.some((c) => c.command === "knowledge_domain_merge")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Merge" }));
    await waitFor(() => expect(calls).toContainEqual({ command: "knowledge_domain_merge", args: { from: "payments", into: "auth" } }));

    fireEvent.click(screen.getByRole("button", { name: "Delete gateway's documents" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toContainEqual({ command: "knowledge_delete_repo", args: { repo: "gateway" } }));
  });

  test("renames a domain, but never offers to rename general", async () => {
    base.domains[1].documents = 1;
    base.documents.push(doc("billing/misc", "general", "howto", "Run it locally"));
    page();
    await into("General");
    fireEvent.click(screen.getByTitle("Actions for General"));
    expect(screen.queryByRole("option", { name: /^Rename/ })).toBeNull();
    fireEvent.keyDown(document, { key: "Escape" });
    fireEvent.click(screen.getByRole("button", { name: "All domains" }));
    await into("Payments");

    fireEvent.click(screen.getByTitle("Actions for Payments"));
    fireEvent.click(screen.getByRole("option", { name: /^Rename/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Money" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(calls).toContainEqual({ command: "knowledge_domain_edit", args: { domain: { id: "payments", name: "Money", description: "Charging and refunds" } } }),
    );
  });

  test("reads the base again when a turn keeps a document, and not on other tool results", async () => {
    page();
    await screen.findByRole("button", { name: /^Payments/ });
    const reads = () => calls.filter((c) => c.command === "knowledge_base").length;
    const before = reads();
    emit("chat:turn-event", { turnId: "t", type: "toolResult", payload: { id: "1", result: { result: "readFile" } } });
    expect(reads()).toBe(before);
    emit("chat:turn-event", { turnId: "t", type: "toolResult", payload: { id: "2", result: { result: "knowledgeWritten" } } });
    await waitFor(() => expect(reads()).toBe(before + 1));
  });
});

describe("splitDocument", () => {
  test("parts the body from the frontmatter's lists", () => {
    const file = "---\nid: a/b\naliases:\n- Saldo\nprovides:\n- api: GET /saldo\nevidence:\n- 'src/a.rs#L1-L2'\nconfidence: confirmed\n---\n# Title\n";
    expect(splitDocument(file)).toEqual({
      body: "# Title\n",
      facts: { provides: ["api: GET /saldo"], consumes: [], evidence: ["src/a.rs#L1-L2"], aliases: ["Saldo"] },
    });
    expect(splitDocument("# No frontmatter\n").body).toBe("# No frontmatter\n");
  });
});
