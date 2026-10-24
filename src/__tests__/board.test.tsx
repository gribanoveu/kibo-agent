import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { Card } from "../lib/board";

// The Board page: the Assistant's cards in three columns, read when the page
// opens and again on `board:changed`; added to To do and edited on the page,
// moved a column at a time, removed from any column, and Done's oldest in an
// archive folded under the columns.

let stored: Card[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let refusal: string | null = null;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "board_list") return Promise.resolve(structuredClone(stored));
    if (command === "board_change") {
      if (refusal) return Promise.reject(refusal);
      const change = args!.change as { op: string; id: string; column?: Card["column"]; title?: string; note?: string; due?: string };
      const text = { title: change.title?.trim() ?? "", note: change.note?.trim() || undefined, due: change.due || undefined };
      stored =
        change.op === "remove"
          ? stored.filter((c) => c.id !== change.id)
          : change.op === "add"
            ? [...stored, { id: `c${stored.length + 1}`, ...text, column: "todo" }]
            : stored.map((c) => (c.id !== change.id ? c : change.op === "edit" ? { ...c, ...text } : { ...c, column: change.column! }));
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
const { dropChange, dueState, localDay, orderOf } = await import("../lib/board");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const column = (name: string) => within(screen.getByRole("region", { name }));

beforeEach(() => {
  calls = [];
  refusal = null;
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

  test("a card is added to To do from its title and note; Enter in the title saves, an empty title cannot", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(column("Doing").queryByText("Add a card")).toBeNull();
    fireEvent.click(column("To do").getByText("Add a card"));
    const form = within(screen.getByRole("group", { name: "New card" }));
    expect((form.getByRole("button", { name: "Add" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(form.getByLabelText("Title"), { target: { value: "Call the bank" } });
    fireEvent.change(form.getByLabelText("Note"), { target: { value: "about the card" } });
    fireEvent.keyDown(form.getByLabelText("Title"), { key: "Enter", code: "Enter" });
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "add", title: "Call the bank", note: "about the card", due: "" } } });
    expect(column("To do").getByText("Call the bank")).toBeTruthy();
    expect(screen.queryByRole("group", { name: "New card" })).toBeNull();
  });

  test("a card is edited in place; Escape leaves it as it was", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(column("Done").getByLabelText("Edit"));
    fireEvent.change(column("Done").getByLabelText("Title"), { target: { value: "Something else" } });
    fireEvent.keyDown(column("Done").getByLabelText("Title"), { key: "Escape", code: "Escape" });
    expect(calls.some((c) => c.command === "board_change")).toBe(false);
    expect(column("Done").getByText("Renew the cert")).toBeTruthy();

    fireEvent.click(column("To do").getByLabelText("Edit"));
    const form = within(screen.getByRole("group", { name: "Edit c1" }));
    expect((form.getByLabelText("Note") as HTMLTextAreaElement).value).toBe("wlbuh");
    fireEvent.change(form.getByLabelText("Title"), { target: { value: "File it today" } });
    fireEvent.change(form.getByLabelText("Note"), { target: { value: "" } });
    fireEvent.click(form.getByRole("button", { name: "Save" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "c1", title: "File it today", note: "", due: "" } } });
    expect(column("To do").getByText("File it today")).toBeTruthy();
    expect(column("To do").queryByText("wlbuh")).toBeNull();
  });

  test("a due day is set in the editor, and marked on an open card once it has come", async () => {
    stored = [
      { id: "c1", title: "Late", due: "2000-01-01", column: "todo" },
      { id: "c2", title: "Now", due: localDay(), column: "doing" },
      { id: "c3", title: "Later", due: "2999-01-01", column: "todo" },
      { id: "c4", title: "Finished late", due: "2000-01-01", column: "done" },
    ];
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    const due = (title: string) => screen.getByText(title).closest("article")!.querySelector(".board-card-due")!;
    expect(due("Late").className).toContain("past");
    expect(due("Late").getAttribute("title")).toStartWith("Overdue");
    expect(due("Now").className).toContain("today");
    expect(due("Later").className).toContain("ahead");
    expect(due("Finished late").className).toContain("ahead");

    fireEvent.click(column("To do").getAllByLabelText("Edit")[0]);
    const form = within(screen.getByRole("group", { name: "Edit c1" }));
    expect((form.getByLabelText("Due") as HTMLInputElement).value).toBe("2000-01-01");
    fireEvent.change(form.getByLabelText("Due"), { target: { value: "2999-06-01" } });
    fireEvent.click(form.getByRole("button", { name: "Save" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "c1", title: "Late", note: "", due: "2999-06-01" } } });
    expect(due("Late").className).toContain("ahead");
  });

  test("a note past its three lines opens in place with Show more, and a short one has none", async () => {
    // happy-dom lays nothing out: a note says it is cut when it holds more than one line.
    const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollHeight");
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
      configurable: true,
      get(this: HTMLElement) {
        return this.classList.contains("board-card-note") && this.textContent!.includes("\n") ? 100 : 0;
      },
    });
    try {
      stored = [
        { id: "c1", title: "Long", note: "one\ntwo\nthree\nfour", column: "todo" },
        { id: "c2", title: "Short", note: "one line", column: "todo" },
      ];
      render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
      await settle();
      const more = column("To do").getAllByRole("button", { name: "Show more" });
      expect(more).toHaveLength(1);
      expect(screen.getByText("one line").closest("article")!.querySelector(".board-card-more")).toBeNull();

      fireEvent.click(more[0]);
      expect(screen.getByText(/four/).className).toContain("open");
      expect(column("To do").getByRole("button", { name: "Show less" }).getAttribute("aria-expanded")).toBe("true");
      fireEvent.click(column("To do").getByRole("button", { name: "Show less" }));
      expect(screen.getByText(/four/).className).not.toContain("open");
    } finally {
      if (height) Object.defineProperty(HTMLElement.prototype, "scrollHeight", height);
      else delete (HTMLElement.prototype as unknown as Record<string, unknown>).scrollHeight;
    }
  });

  test("a refused card keeps its editor open and says why", async () => {
    render(<BoardPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    refusal = "the board is full (200 cards)";
    fireEvent.click(column("To do").getByText("Add a card"));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "One more" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await settle();
    expect(screen.getByText(refusal)).toBeTruthy();
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("One more");
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
    expect(screen.getByText(/Add a card, or ask the Assistant to remind you/)).toBeTruthy();
  });
});

describe("a due day", () => {
  test("is the user's own calendar day, padded", () => {
    expect(localDay(new Date(2026, 0, 5, 23, 59))).toBe("2026-01-05");
    expect(localDay(new Date(2026, 10, 30, 0, 1))).toBe("2026-11-30");
  });

  test("has come when it is today or before", () => {
    expect(dueState("2026-10-23", "2026-10-24")).toBe("past");
    expect(dueState("2026-10-24", "2026-10-24")).toBe("today");
    expect(dueState("2026-10-25", "2026-10-24")).toBe("ahead");
    expect(dueState("2025-12-31", "2026-01-01")).toBe("past");
  });
});

describe("a dropped card", () => {
  const order = { todo: ["c1", "c2", "c3"], doing: ["c4"], done: ["c5"] };

  test("is read top to bottom per open column, the archive left out", () => {
    const card = (id: string, column: Card["column"]): Card => ({ id, title: id, column });
    expect(orderOf([card("c1", "done"), card("c2", "todo"), card("c3", "archived"), card("c4", "todo")])).toEqual({ todo: ["c2", "c4"], doing: [], done: ["c1"] });
  });

  test("moves to the column and place the drag left it in", () => {
    expect(dropChange(order, { ...order, todo: ["c2", "c3", "c1"] }, "c1")).toEqual({ op: "move", id: "c1", column: "todo", index: 2 });
    expect(dropChange(order, { ...order, todo: ["c1", "c3"], doing: ["c2", "c4"] }, "c2")).toEqual({ op: "move", id: "c2", column: "doing", index: 0 });
  });

  test("back where it started, or never in the order, sends nothing", () => {
    expect(dropChange(order, order, "c2")).toBeNull();
    expect(dropChange(order, { ...order, todo: ["c1", "c3"], doing: ["c4"] }, "c2")).toBeNull();
  });
});
