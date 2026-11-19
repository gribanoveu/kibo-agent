import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { Card } from "../lib/board";

// The Board page: the Assistant's cards in three columns, read when the page
// opens and again on `board:changed`; added to To do and edited in a card's sheet,
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
      const change = args!.change as { op: string; id: string; column?: Card["column"]; title?: string; note?: string; due?: string; text?: string; at?: string; comment?: number };
      if (change.op === "comment" || change.op === "uncomment") {
        stored = stored.map((c) => {
          if (c.id !== change.id) return c;
          const comments = c.comments ?? [];
          return change.op === "comment"
            ? { ...c, comments: [...comments, { id: comments.length + 1, text: change.text!.trim(), at: change.at! }] }
            : { ...c, comments: comments.filter((m) => m.id !== change.comment) };
        });
        return Promise.resolve(structuredClone(stored));
      }
      // What an edit leaves out stays, as `domain::board` keeps it.
      const given = (key: "title" | "note" | "due") => key in change;
      const text = {
        ...(given("title") && { title: change.title!.trim() }),
        ...(given("note") && { note: change.note!.trim() || undefined }),
        ...(given("due") && { due: change.due || undefined }),
      } as Partial<Card>;
      stored =
        change.op === "remove"
          ? stored.filter((c) => c.id !== change.id)
          : change.op === "add"
            ? [{ id: `c${stored.length + 1}`, title: "", ...text, column: "todo" }, ...stored]
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
const { ago } = await import("../components/BoardCardSheet");
const { IS_MAC } = await import("../lib/shortcuts");
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
    render(<BoardPage openPane={() => {}} />);
    await settle();
    expect(column("To do").getByText("File the memory request")).toBeTruthy();
    expect(column("To do").getByText("wlbuh")).toBeTruthy();
    expect(column("Done").getByText("Renew the cert")).toBeTruthy();
  });

  test("a card moves a column at a time, and is removed from any column", async () => {
    render(<BoardPage openPane={() => {}} />);
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
    render(<BoardPage openPane={() => {}} />);
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
    render(<BoardPage openPane={() => {}} />);
    await settle();
    expect(column("Doing").queryByLabelText("Add a card")).toBeNull();
    fireEvent.click(column("To do").getByLabelText("Add a card"));
    const form = within(screen.getByRole("group", { name: "New card" }));
    // Written above To do's cards, where it lands.
    const editor = screen.getByRole("group", { name: "New card" });
    expect(editor.compareDocumentPosition(screen.getByText("File the memory request")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect((form.getByRole("button", { name: "Add" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.change(form.getByLabelText("Title"), { target: { value: "Call the bank" } });
    fireEvent.change(form.getByLabelText("Note"), { target: { value: "about the card" } });
    fireEvent.keyDown(form.getByLabelText("Title"), { key: "Enter", code: "Enter" });
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "add", title: "Call the bank", note: "about the card", due: "" } } });
    expect(screen.getByRole("region", { name: "To do" }).querySelector("[data-card-id]")!.textContent).toContain("Call the bank");
    expect(screen.queryByRole("group", { name: "New card" })).toBeNull();
  });

  test("a click on a card opens it in a sheet, where its title and description are edited in place", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const sheet = within(screen.getByRole("complementary", { name: "c1 details" }));
    expect(column("To do").getByText("File the memory request").closest("article")!.className).toContain("selected");

    // Escape puts it back, and leaving it then saves nothing.
    const title = sheet.getByLabelText("Title") as HTMLInputElement;
    fireEvent.change(title, { target: { value: "Something else" } });
    fireEvent.keyDown(title, { key: "Escape", code: "Escape" });
    fireEvent.blur(title);
    expect(title.value).toBe("File the memory request");
    expect(calls.some((c) => c.command === "board_change")).toBe(false);
    expect(screen.getByRole("complementary", { name: "c1 details" })).toBeTruthy();

    // An empty title goes back to what it was.
    fireEvent.change(title, { target: { value: "  " } });
    fireEvent.blur(title);
    expect(title.value).toBe("File the memory request");
    expect(calls.some((c) => c.command === "board_change")).toBe(false);

    fireEvent.change(title, { target: { value: "File it today" } });
    fireEvent.blur(title);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "c1", title: "File it today" } } });
    expect(column("To do").getByText("File it today")).toBeTruthy();

    // Read as text until clicked into.
    expect(sheet.queryByLabelText("Description")).toBeNull();
    fireEvent.click(sheet.getByRole("button", { name: "Edit description" }));
    const note = sheet.getByLabelText("Description") as HTMLTextAreaElement;
    expect(note.value).toBe("wlbuh");
    fireEvent.change(note, { target: { value: "" } });
    fireEvent.blur(note);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "c1", note: "" } } });
    expect(column("To do").queryByText("wlbuh")).toBeNull();
    expect(sheet.getByText("Add a description…")).toBeTruthy();
  });

  test("the sheet moves a card by its status, removes it, and closes with it", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByLabelText("Open"));
    const sheet = within(screen.getByRole("complementary", { name: "c1 details" }));
    fireEvent.click(sheet.getByRole("button", { name: "To do" }));
    fireEvent.click(sheet.getByRole("option", { name: "Doing" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "move", id: "c1", column: "doing" } } });
    expect(column("Doing").getByText("File the memory request")).toBeTruthy();

    // Another card opens in the same sheet.
    fireEvent.pointerDown(screen.getByText("Renew the cert"));
    fireEvent.click(screen.getByText("Renew the cert"));
    expect(screen.getByRole("complementary", { name: "c2 details" })).toBeTruthy();

    fireEvent.click(within(screen.getByRole("complementary", { name: "c2 details" })).getByRole("button", { name: "More" }));
    // A press in the menu, drawn outside the sheet, is not one that closes it.
    fireEvent.pointerDown(screen.getByRole("menuitem", { name: "Remove" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "c2" } } });
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  test("a due day is set in the card sheet, and marked on an open card once it has come", async () => {
    stored = [
      { id: "c1", title: "Late", due: "2000-01-01", column: "todo" },
      { id: "c2", title: "Now", due: localDay(), column: "doing" },
      { id: "c3", title: "Later", due: "2999-01-01", column: "todo" },
      { id: "c4", title: "Finished late", due: "2000-01-01", column: "done" },
    ];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    const due = (title: string) => screen.getByText(title, { selector: ".board-card-title" }).closest("article")!.querySelector(".board-card-due")!;
    expect(due("Late").className).toContain("past");
    expect(due("Late").getAttribute("title")).toStartWith("Overdue");
    expect(due("Now").className).toContain("today");
    expect(due("Later").className).toContain("ahead");
    expect(due("Finished late").className).toContain("ahead");

    fireEvent.click(column("To do").getByText("Late"));
    const sheet = within(screen.getByRole("complementary", { name: "c1 details" }));
    expect((sheet.getByLabelText("Due") as HTMLInputElement).value).toBe("2000-01-01");
    expect(sheet.getByText((_, el) => el?.classList.contains("board-sheet-due-when") ?? false).className).toContain("past");
    fireEvent.change(sheet.getByLabelText("Due"), { target: { value: "2999-06-01" } });
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "c1", due: "2999-06-01" } } });
    expect(sheet.getByText((_, el) => el?.classList.contains("board-sheet-due-when") ?? false).className).toContain("ahead");
    expect(due("Late").className).toContain("ahead");

    // A finished card's day is not marked.
    fireEvent.click(column("Done").getByText("Finished late"));
    const done = within(screen.getByRole("complementary", { name: "c4 details" }));
    expect(done.getByText((_, el) => el?.classList.contains("board-sheet-due-when") ?? false).className).toContain("ahead");
  });

  test("a comment is written under the card, the newest first, counted on the card, and deleted", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const sheet = within(screen.getByRole("complementary", { name: "c1 details" }));
    const box = sheet.getByLabelText("New comment") as HTMLTextAreaElement;
    fireEvent.focus(box);
    expect((sheet.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);

    const before = Date.now();
    fireEvent.change(box, { target: { value: "asked the bank" } });
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", ...(IS_MAC ? { metaKey: true } : { ctrlKey: true }) });
    await settle();
    const sent = calls.at(-1)!.args!.change as { op: string; id: string; text: string; at: string };
    expect({ ...sent, at: undefined }).toEqual({ op: "comment", id: "c1", text: "asked the bank", at: undefined });
    expect(new Date(sent.at).getTime()).toBeGreaterThanOrEqual(before - 1000);
    expect(box.value).toBe("");

    fireEvent.change(box, { target: { value: "they call back on **Monday**" } });
    fireEvent.click(sheet.getByRole("button", { name: "Save" }));
    await settle();
    const texts = [...sheet.getByRole("region", { name: "Comments" }).querySelectorAll(".board-sheet-comment-text")].map((t) => t.textContent);
    expect(texts).toEqual(["they call back on Monday", "asked the bank"]);
    expect(sheet.getByText("Monday").tagName).toBe("STRONG");
    expect(column("To do").getByTitle("2 comments")).toBeTruthy();

    fireEvent.click(sheet.getAllByRole("button", { name: "Delete the comment" })[1]);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "uncomment", id: "c1", comment: 1 } } });
    expect(sheet.queryByText("asked the bank")).toBeNull();
    expect(column("To do").getByTitle("1 comments")).toBeTruthy();
  });

  test("a refused comment stays in its box", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const sheet = within(screen.getByRole("complementary", { name: "c1 details" }));
    refusal = "the card has 100 comments — remove the ones no longer needed";
    fireEvent.change(sheet.getByLabelText("New comment"), { target: { value: "one more" } });
    fireEvent.click(sheet.getByRole("button", { name: "Save" }));
    await settle();
    expect(sheet.getByText(refusal)).toBeTruthy();
    expect((sheet.getByLabelText("New comment") as HTMLTextAreaElement).value).toBe("one more");
  });

  test("a description is read as Markdown, and a link in it is followed rather than edited", async () => {
    stored = [{ id: "c1", title: "Read", note: "See **the spec** at [docs](https://example.com)", column: "todo" }];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("Read"));
    const sheet = within(screen.getByRole("complementary", { name: "c1 details" }));
    expect(sheet.getByText("the spec").tagName).toBe("STRONG");
    fireEvent.click(sheet.getByText("docs"));
    expect(sheet.queryByLabelText("Description")).toBeNull();
    fireEvent.click(sheet.getByText("the spec"));
    expect((sheet.getByLabelText("Description") as HTMLTextAreaElement).value).toBe("See **the spec** at [docs](https://example.com)");
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
      render(<BoardPage openPane={() => {}} />);
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
    render(<BoardPage openPane={() => {}} />);
    await settle();
    refusal = "the board is full (200 cards)";
    fireEvent.click(column("To do").getByLabelText("Add a card"));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "One more" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await settle();
    expect(screen.getByText(refusal)).toBeTruthy();
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("One more");
  });

  test("a card the model added appears when the backend says the board changed", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    stored = [...stored, { id: "c3", title: "Call the bank", column: "todo" }];
    act(() => listeners.forEach((handler) => handler()));
    await settle();
    expect(column("To do").getByText("Call the bank")).toBeTruthy();
  });

  test("a board with only an archive says how cards get there", async () => {
    stored = [{ id: "c3", title: "Old one", column: "archived" }];
    render(<BoardPage openPane={() => {}} />);
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

describe("when a comment was written", () => {
  const now = new Date("2026-11-19T12:00:00Z").getTime();
  const words = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

  test("is said in the largest whole unit up to a week, then as the day", () => {
    expect(ago("2026-11-19T11:59:30Z", now)).toBe(words.format(0, "second"));
    expect(ago("2026-11-19T11:55:00Z", now)).toBe(words.format(-5, "minute"));
    expect(ago("2026-11-19T10:30:00Z", now)).toBe(words.format(-1, "hour"));
    expect(ago("2026-11-19T09:00:00Z", now)).toBe(words.format(-3, "hour"));
    expect(ago("2026-11-18T11:00:00Z", now)).toBe(words.format(-1, "day"));
    expect(ago("2026-11-13T12:00:00Z", now)).toBe(words.format(-6, "day"));
    expect(ago("2026-11-01T12:00:00Z", now)).toBe(new Date("2026-11-01T12:00:00Z").toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }));
  });
});
