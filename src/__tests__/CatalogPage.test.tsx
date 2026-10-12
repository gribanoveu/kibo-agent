import { describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CatalogEntry, CatalogSource } from "../lib/catalog";

let answer: CatalogSource[] = [];
let asked = 0;
// Every action asked of the backend, but the list.
let calls: [string, unknown][] = [];
let failInstall: string | null = null;
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    if (command === "catalog_list") {
      asked += 1;
      return Promise.resolve(answer);
    }
    calls.push([command, args]);
    if (command === "catalog_preview") return Promise.resolve("# Review\n\nRead the **diff**.");
    if (command === "catalog_install" && failInstall) return Promise.reject(failInstall);
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const { CatalogPage } = await import("../components/CatalogPage");

const now = Math.floor(Date.now() / 1000);

function entry(over: Partial<CatalogEntry> & Pick<CatalogEntry, "id" | "kind" | "name">): CatalogEntry {
  return {
    description: "",
    rev: "1",
    path: `${over.kind}s/${over.id}`,
    files: [],
    state: "available",
    target: `~/.kibo/${over.kind}s/${over.id}`,
    replacesBuiltIn: false,
    ...over,
  };
}
const withItems = (...items: CatalogEntry[]): CatalogSource => ({ ...own, problems: [], items });
const own: CatalogSource = {
  url: "https://raw.githubusercontent.com/gribanoveu/kibo-agent-catalog/main/",
  builtIn: true,
  items: [
    entry({ id: "review", kind: "skill", name: "Code review", description: "Reviews a change", path: "skills/review", files: ["SKILL.md"] }),
    entry({ id: "github", kind: "mcp", name: "GitHub", description: "Issues and PRs", path: "mcp/github.json", target: null }),
  ],
  problems: ["x: \"agent\" is not a kind this app installs"],
  error: null,
  fetchedAt: now,
};

const skill = (id: string) => entry({ id, kind: "skill", name: `Skill ${id}`, files: ["SKILL.md"] });

describe("the catalog page", () => {
  test("lists entries as cards by kind, each with its source, and what was left out", async () => {
    answer = [own];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    expect(await screen.findByText("Code review")).toBeTruthy();
    expect(screen.getAllByText("gribanoveu/kibo-agent-catalog")).toHaveLength(2);
    expect(screen.getByRole("region", { name: "Skills" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "MCP servers" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Hooks" })).toBeNull();
    expect(screen.getByText("1 entry left out")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Install Code review" }) as HTMLButtonElement).disabled).toBe(false);
    // MCP installs in CT-3: its + is there, and off.
    expect((screen.getByRole("button", { name: "Install GitHub" }) as HTMLButtonElement).disabled).toBe(true);
  });

  test("a tab lists one kind", async () => {
    answer = [own];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    await screen.findByText("Code review");
    fireEvent.click(screen.getByRole("tab", { name: "MCP servers" }));
    expect(screen.queryByText("Code review")).toBeNull();
    expect(screen.getByText("GitHub")).toBeTruthy();
  });

  test("search narrows by name, id or description, and says when nothing matches", async () => {
    answer = [own];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    await screen.findByText("Code review");
    const search = screen.getByLabelText("Search the catalog");
    fireEvent.change(search, { target: { value: "  PRS " } });
    expect(screen.queryByText("Code review")).toBeNull();
    expect(screen.getByText("GitHub")).toBeTruthy();
    fireEvent.change(search, { target: { value: "review" } });
    expect(screen.getByText("Code review")).toBeTruthy();
    expect(screen.queryByText("GitHub")).toBeNull();
    fireEvent.change(search, { target: { value: "nothing like it" } });
    expect(screen.getByText("Nothing matches “nothing like it”.")).toBeTruthy();
  });

  test("All shows a few of a long kind, and Show all opens its tab", async () => {
    answer = [{ ...own, items: ["a", "b", "c", "d", "e"].map(skill) }];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    await screen.findByText("Skill a");
    expect(screen.queryByText("Skill e")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Show all 5/ }));
    expect(screen.getByText("Skill e")).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Skills" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.queryByRole("button", { name: /Show all/ })).toBeNull();
  });

  test("says when the list is the copy kept from last time, and asks again on Refresh", async () => {
    answer = [{ ...own, error: "the catalog answered 503", url: "https://cat.example/main/", builtIn: false }];
    asked = 0;
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    expect(await screen.findByText(/cat\.example\/main: showing the copy kept from last time — the catalog answered 503/)).toBeTruthy();
    expect(screen.getByText("Code review")).toBeTruthy();
    await waitFor(() => expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(asked).toBe(2));
  });

  test("an unreachable source with nothing kept shows only why", async () => {
    answer = [{ ...own, items: [], problems: [], error: "the catalog could not be reached: no route", fetchedAt: null }];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    expect(await screen.findByText(/the catalog could not be reached: no route/)).toBeTruthy();
    expect(screen.queryByText(/showing the copy/)).toBeNull();
    expect(screen.queryByText(/Nothing/)).toBeNull();
  });

  test("+ installs, and the list is read again to show it", async () => {
    answer = [own];
    calls = [];
    asked = 0;
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    await screen.findByText("Code review");
    answer = [withItems({ ...own.items[0], state: "installed" }, own.items[1])];
    fireEvent.click(screen.getByRole("button", { name: "Install Code review" }));
    expect(await screen.findByLabelText("Installed")).toBeTruthy();
    expect(calls).toEqual([["catalog_install", { source: own.url, kind: "skill", id: "review" }]]);
    expect(asked).toBe(2);
    // The + did not open the panel: it is an action of its own.
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  test("a card opens its details beside the page, with the text it installs", async () => {
    answer = [withItems({ ...own.items[0], files: ["SKILL.md", "checklist.md"] })];
    calls = [];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    fireEvent.click(await screen.findByRole("button", { name: "Code review" }));
    const panel = screen.getByRole("complementary", { name: "Code review details" });
    expect(within(panel).getByText("~/.kibo/skills/review")).toBeTruthy();
    expect(within(panel).getByText("checklist.md")).toBeTruthy();
    expect(await within(panel).findByText("diff")).toBeTruthy();
    expect(calls).toEqual([["catalog_preview", { source: own.url, kind: "skill", id: "review" }]]);

    fireEvent.click(within(panel).getByRole("button", { name: "Install" }));
    await waitFor(() => expect(calls.at(-1)?.[0]).toBe("catalog_install"));
    fireEvent.click(within(panel).getByRole("button", { name: "Close panel" }));
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  test("a failed install says why in the panel", async () => {
    answer = [own];
    failInstall = "the catalog answered 404";
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    fireEvent.click(await screen.findByRole("button", { name: "Code review" }));
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    expect(await screen.findByText("the catalog answered 404")).toBeTruthy();
    failInstall = null;
  });

  test("Remove asks first only when the installed copy was changed here", async () => {
    answer = [withItems(entry({ id: "changelog", kind: "command", name: "/changelog", state: "installed" }), entry({ id: "tls", kind: "runbook", name: "TLS", state: "changed", role: "kubernetes" }))];
    calls = [];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    fireEvent.click(await screen.findByRole("button", { name: "/changelog" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(calls.filter(([c]) => c === "catalog_remove")).toHaveLength(1));

    fireEvent.click(screen.getByRole("button", { name: "TLS" }));
    expect(screen.getByText("Installed, and changed here since")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    const confirm = screen.getByRole("dialog", { name: "Remove TLS?" });
    expect(calls.filter(([c]) => c === "catalog_remove")).toHaveLength(1);
    fireEvent.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(calls.filter(([c]) => c === "catalog_remove")).toHaveLength(2));
    expect(calls.at(-1)).toEqual(["catalog_remove", { source: own.url, kind: "runbook", id: "tls" }]);
  });

  test("something else in the place is not installed over, and a built-in runbook's name is said", async () => {
    answer = [withItems(entry({ id: "changelog", kind: "command", name: "/changelog", state: "taken" }), entry({ id: "crashloop", kind: "runbook", name: "Crash loop", role: "kubernetes", replacesBuiltIn: true }))];
    render(<CatalogPage openPane={() => {}} sideHandle={<div />} />);
    fireEvent.click(await screen.findByRole("button", { name: "/changelog" }));
    expect((screen.getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/was not installed from this catalog/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Install /changelog" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Crash loop" }));
    expect(screen.getByText(/The app ships a runbook named crashloop/)).toBeTruthy();
  });
});
