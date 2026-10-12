import { describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { CatalogSource } from "../lib/catalog";

let answer: CatalogSource[] = [];
let asked = 0;
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    if (command === "catalog_list") asked += 1;
    return Promise.resolve(answer);
  },
  transformCallback: (callback: unknown) => callback,
}));

const { CatalogPage } = await import("../components/CatalogPage");

const now = Math.floor(Date.now() / 1000);
const own: CatalogSource = {
  url: "https://raw.githubusercontent.com/gribanoveu/kibo-agent-catalog/main/",
  builtIn: true,
  items: [
    { id: "review", kind: "skill", name: "Code review", description: "Reviews a change", rev: "1", path: "skills/review", files: ["SKILL.md"] },
    { id: "github", kind: "mcp", name: "GitHub", description: "Issues and PRs", rev: "1", path: "mcp/github.json", files: [] },
  ],
  problems: ["x: \"agent\" is not a kind this app installs"],
  error: null,
  fetchedAt: now,
};

const skill = (id: string) => ({ id, kind: "skill" as const, name: `Skill ${id}`, description: "", rev: "1", path: `skills/${id}`, files: ["SKILL.md"] });

describe("the catalog page", () => {
  test("lists entries as cards by kind, each with its source, and what was left out", async () => {
    answer = [own];
    render(<CatalogPage openPane={() => {}} />);
    expect(await screen.findByText("Code review")).toBeTruthy();
    expect(screen.getAllByText("gribanoveu/kibo-agent-catalog")).toHaveLength(2);
    expect(screen.getByRole("region", { name: "Skills" })).toBeTruthy();
    expect(screen.getByRole("region", { name: "MCP servers" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Hooks" })).toBeNull();
    expect(screen.getByText("1 entry left out")).toBeTruthy();
    // Installing comes in CT-2: the button is there, and does nothing yet.
    expect((screen.getByRole("button", { name: "Install Code review" }) as HTMLButtonElement).disabled).toBe(true);
  });

  test("a tab lists one kind", async () => {
    answer = [own];
    render(<CatalogPage openPane={() => {}} />);
    await screen.findByText("Code review");
    fireEvent.click(screen.getByRole("tab", { name: "MCP servers" }));
    expect(screen.queryByText("Code review")).toBeNull();
    expect(screen.getByText("GitHub")).toBeTruthy();
  });

  test("search narrows by name, id or description, and says when nothing matches", async () => {
    answer = [own];
    render(<CatalogPage openPane={() => {}} />);
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
    render(<CatalogPage openPane={() => {}} />);
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
    render(<CatalogPage openPane={() => {}} />);
    expect(await screen.findByText(/cat\.example\/main: showing the copy kept from last time — the catalog answered 503/)).toBeTruthy();
    expect(screen.getByText("Code review")).toBeTruthy();
    await waitFor(() => expect((screen.getByRole("button", { name: "Refresh" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(asked).toBe(2));
  });

  test("an unreachable source with nothing kept shows only why", async () => {
    answer = [{ ...own, items: [], problems: [], error: "the catalog could not be reached: no route", fetchedAt: null }];
    render(<CatalogPage openPane={() => {}} />);
    expect(await screen.findByText(/the catalog could not be reached: no route/)).toBeTruthy();
    expect(screen.queryByText(/showing the copy/)).toBeNull();
    expect(screen.queryByText(/Nothing/)).toBeNull();
  });
});
