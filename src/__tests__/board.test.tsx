import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { Card } from "../lib/board";

// The Board page: the Assistant's cards in three columns, read when the page
// opens and again on `board:changed`; moved a column at a time, removed from
// any column, and Done's oldest in an archive folded under the columns.

let stored: Card[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "board_list") return Promise.resolve(structuredClone(stored));
    if (command === "board_change") {
      const change = args!.change as { op: string; id: string; column?: Card["column"] };
      stored =
        change.op === "remove"
          ? stored.filter((c) => c.id !== change.id)
          : stored.map((c) => (c.id === change.id ? { ...c, column: change.column! } : c));
      return Promise.resolve(structuredClone(stored));
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Set<() => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: () => void) => {
    if (channel === "board:changed") listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { BoardPage } = await import("../components/BoardPage");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const column = (name: string) => within(screen.getByRole("region", { name }));

beforeEach(() => {
  calls = [];
  stored = [
    { id: "c1", title: "File the memory request", note: "wlbuh", column: "todo" },
    { id: "c2", title: "Renew the cert", column: "done" },
    { id: "c3", title: "Old one", column: "archived" },
    { id: "c4", title: "Older one", column: "archived" },
  ];
});

describe("the board page", () => {
  test("lists each card in its column, note included", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(column("To do").getByText("File the memory request")).toBeTruthy();
    expect(column("To do").getByText("wlbuh")).toBeTruthy();
    expect(column("Done").getByText("Renew the cert")).toBeTruthy();
  });

  test("a card moves a column at a time, and is removed from any column", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(column("To do").queryByLabelText("Move to Done")).toBeNull();
    expect(column("Done").queryByLabelText("Move to Archive")).toBeNull();

    fireEvent.click(column("To do").getByLabelText("Move to Doing"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "move", id: "c1", column: "doing" } } });
    expect(column("Doing").getByText("File the memory request")).toBeTruthy();

    fireEvent.click(column("Done").getByLabelText("Remove"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "c2" } } });
    expect(screen.queryByText("Renew the cert")).toBeNull();

    fireEvent.click(column("Doing").getByLabelText("Remove"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "c1" } } });
  });

  test("the archive is folded under the columns, newest first, and its cards can be removed", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.queryByText("Old one")).toBeNull();
    const toggle = column("Archive").getByRole("button", { name: /Archive/ });
    expect(toggle.textContent).toContain("2");

    fireEvent.click(toggle);
    const titles = [...column("Archive").getAllByRole("article")].map((a) => a.querySelector(".board-card-title")?.textContent);
    expect(titles).toEqual(["Older one", "Old one"]);
    expect(column("Archive").queryByLabelText(/Move to/)).toBeNull();

    fireEvent.click(column("Archive").getAllByLabelText("Remove")[0]);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "c4" } } });
  });

  test("a card the model added appears when the backend says the board changed", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    stored = [...stored, { id: "c3", title: "Call the bank", column: "todo" }];
    act(() => listeners.forEach((handler) => handler()));
    await settle();
    expect(column("To do").getByText("Call the bank")).toBeTruthy();
  });

  test("a board with only an archive says how cards get there", async () => {
    stored = [{ id: "c3", title: "Old one", column: "archived" }];
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.getByText(/Ask the Assistant to remind you/)).toBeTruthy();
  });
});
