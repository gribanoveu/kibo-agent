import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { KnowledgeBase, KnowledgeDocument } from "../lib/knowledge";

// The Knowledge page (docs/40-knowledge-base.md, decision 9): the base by
// domain, then repository, then document; a document opens on the right with
// its evidence, and the page's changes answer with the base after them.

let base: KnowledgeBase;
let calls: { command: string; args?: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "knowledge_base") return Promise.resolve(structuredClone(base));
    if (command === "knowledge_document") return Promise.resolve(`---\nid: ${args!.id}\nconfidence: confirmed\n---\n# Body of ${args!.id}\n`);
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
const { documentMarkdown } = await import("../lib/knowledge");

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
  render(<KnowledgePage openPane={() => {}} sideHandle={<span />} openPage={() => {}} openChat={() => {}} removeChat={() => Promise.resolve(true)} chats={[]} />);
const titles = (repo: string) =>
  within(screen.getByRole("listbox", { name: `${repo}'s documents in Payments` }))
    .getAllByRole("option")
    .map((o) => o.querySelector(".knowledge-title")?.textContent);

describe("KnowledgePage", () => {
  test("lists each domain's repositories and their documents by kind, with their marks", async () => {
    page();
    await waitFor(() => expect(titles("billing")).toEqual(["Billing service", "Refunds API"]));
    expect(titles("gateway")).toEqual(["Charge retry"]);
    const payments = screen.getByRole("region", { name: "Payments" });
    expect(payments.textContent).toContain("3 documents · 2 repositories");
    expect(payments.textContent).toContain("1 stale");
    expect(screen.getByRole("option", { name: /Refunds API/ }).textContent).toContain("stale");
    expect(screen.getByRole("option", { name: /Charge retry/ }).textContent).toContain("no evidence");
    // An empty "general" is left out; another empty domain is shown, to be merged or described.
    expect(screen.queryByRole("region", { name: "General" })).toBeNull();
    expect(screen.getByRole("region", { name: "Auth" }).textContent).toContain("No documents in it.");
  });

  test("opens a document with its frontmatter, moves it and deletes it", async () => {
    page();
    await waitFor(() => screen.getByRole("option", { name: /Refunds API/ }));
    fireEvent.click(screen.getByRole("option", { name: /Refunds API/ }));
    await waitFor(() => expect(screen.getByText("Body of billing/refunds-api")).toBeTruthy());
    expect(calls).toContainEqual({ command: "knowledge_document", args: { id: "billing/refunds-api" } });

    fireEvent.click(screen.getByTitle("Move it to another domain"));
    fireEvent.click(screen.getByRole("option", { name: /^Auth/ }));
    await waitFor(() => expect(calls).toContainEqual({ command: "knowledge_move", args: { id: "billing/refunds-api", domain: "auth" } }));
    await waitFor(() => expect(screen.getByRole("listbox", { name: "billing's documents in Auth" })).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: "Delete document" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls).toContainEqual({ command: "knowledge_delete", args: { id: "billing/refunds-api" } }));
    await waitFor(() => expect(screen.queryByText("Body of billing/refunds-api")).toBeNull());
  });

  test("merges a domain into another and deletes a repository, each after asking", async () => {
    page();
    await waitFor(() => screen.getByRole("region", { name: "Payments" }));
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
    await waitFor(() => screen.getByRole("region", { name: "General" }));
    fireEvent.click(screen.getByTitle("Actions for General"));
    expect(screen.queryByRole("option", { name: /^Rename/ })).toBeNull();
    fireEvent.keyDown(document, { key: "Escape" });

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
    await waitFor(() => screen.getByRole("region", { name: "Payments" }));
    const reads = () => calls.filter((c) => c.command === "knowledge_base").length;
    const before = reads();
    emit("chat:turn-event", { turnId: "t", type: "toolResult", payload: { id: "1", result: { result: "readFile" } } });
    expect(reads()).toBe(before);
    emit("chat:turn-event", { turnId: "t", type: "toolResult", payload: { id: "2", result: { result: "knowledgeWritten" } } });
    await waitFor(() => expect(reads()).toBe(before + 1));
  });
});

describe("documentMarkdown", () => {
  test("puts the frontmatter in a YAML block over the body", () => {
    expect(documentMarkdown("---\nid: a/b\n---\n# Title\n")).toBe("```yaml\nid: a/b\n```\n\n# Title\n");
    expect(documentMarkdown("# No frontmatter\n")).toBe("# No frontmatter\n");
  });
});
