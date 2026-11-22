import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Card } from "../lib/board";

// The Board page: the Assistant's cards in three columns, read when the page
// opens and again on `board:changed`; added to To do and edited in a card's sheet,
// moved a column at a time, removed from any column, and Done's oldest in an
// archive folded under the columns.

let stored: Card[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let refusal: string | null = null;
let features: { id: string; on: boolean }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "board_list") return Promise.resolve(structuredClone(stored));
    if (command === "features_list") return Promise.resolve(features);
    if (command === "board_change") {
      if (refusal) return Promise.reject(refusal);
      const change = args!.change as { op: string; id: string; column?: Card["column"]; title?: string; note?: string; due?: string; text?: string; at?: string; comment?: number; tags?: string[] };
      if (change.op === "tags") {
        if (refusal) return Promise.reject(refusal);
        const tags = (change as unknown as { tags: string[] }).tags.map((t) => t.trim()).filter(Boolean);
        stored = stored.map((c) => (c.id === change.id ? { ...c, tags } : c));
        return Promise.resolve(structuredClone(stored));
      }
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
            ? [{ id: `T${stored.length + 1}`, title: "", ...text, ...(change.tags && { tags: change.tags }), column: "todo" }, ...stored]
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
const { dropChange, dueState, filterOrder, hasAnyTag, localDay, orderOf, tagsOf } = await import("../lib/board");
const { ago, tagChoices } = await import("../components/BoardCardSheet");
const { IS_MAC } = await import("../lib/shortcuts");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const column = (name: string) => within(screen.getByRole("region", { name }));

beforeEach(() => {
  calls = [];
  refusal = null;
  features = [];
  stored = [
    { id: "T1", title: "File the memory request", note: "wlbuh", column: "todo" },
    { id: "T2", title: "Renew the cert", column: "done" },
    { id: "T3", title: "Old one", column: "archived" },
    { id: "T4", title: "Older one", column: "archived" },
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
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "move", id: "T1", column: "doing" } } });
    expect(column("Doing").getByText("File the memory request")).toBeTruthy();

    fireEvent.click(column("Done").getByLabelText("Remove"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "T2" } } });
    expect(screen.queryByText("Renew the cert")).toBeNull();

    fireEvent.click(column("Doing").getByLabelText("Remove"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "T1" } } });
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
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "T4" } } });
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
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    expect(column("To do").getByText("File the memory request").closest("article")!.className).toContain("selected");

    // Escape puts it back, and leaving it then saves nothing.
    const title = sheet.getByLabelText("Title") as HTMLInputElement;
    fireEvent.change(title, { target: { value: "Something else" } });
    fireEvent.keyDown(title, { key: "Escape", code: "Escape" });
    fireEvent.blur(title);
    expect(title.value).toBe("File the memory request");
    expect(calls.some((c) => c.command === "board_change")).toBe(false);
    expect(screen.getByRole("complementary", { name: "T1 details" })).toBeTruthy();

    // An empty title goes back to what it was.
    fireEvent.change(title, { target: { value: "  " } });
    fireEvent.blur(title);
    expect(title.value).toBe("File the memory request");
    expect(calls.some((c) => c.command === "board_change")).toBe(false);

    fireEvent.change(title, { target: { value: "File it today" } });
    fireEvent.blur(title);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "T1", title: "File it today" } } });
    expect(column("To do").getByText("File it today")).toBeTruthy();

    // Read as text until clicked into.
    expect(sheet.queryByLabelText("Description")).toBeNull();
    fireEvent.click(sheet.getByRole("button", { name: "Edit description" }));
    const note = sheet.getByLabelText("Description") as HTMLTextAreaElement;
    expect(note.value).toBe("wlbuh");
    fireEvent.change(note, { target: { value: "" } });
    fireEvent.blur(note);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "T1", note: "" } } });
    expect(column("To do").queryByText("wlbuh")).toBeNull();
    expect(sheet.getByText("Add a description…")).toBeTruthy();
  });

  test("a description's line breaks stay line breaks in the sheet, as typed", async () => {
    stored = [{ id: "T1", title: "Lines", note: "first line\nsecond line", column: "todo" }];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("Lines"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    const view = sheet.getByRole("button", { name: "Edit description" });
    expect(view.querySelectorAll("br")).toHaveLength(1);
    expect(view.textContent).toBe("first line\nsecond line");
    // Clicked into, the box holds the lines as they were typed.
    fireEvent.click(view);
    expect((sheet.getByLabelText("Description") as HTMLTextAreaElement).value).toBe("first line\nsecond line");
  });

  test("the sheet moves a card by its status, removes it, and closes with it", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByLabelText("Open"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    fireEvent.click(sheet.getByRole("button", { name: "To do" }));
    fireEvent.click(sheet.getByRole("option", { name: "Doing" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "move", id: "T1", column: "doing" } } });
    expect(column("Doing").getByText("File the memory request")).toBeTruthy();

    // Another card opens in the same sheet.
    fireEvent.pointerDown(screen.getByText("Renew the cert"));
    fireEvent.click(screen.getByText("Renew the cert"));
    expect(screen.getByRole("complementary", { name: "T2 details" })).toBeTruthy();

    fireEvent.click(within(screen.getByRole("complementary", { name: "T2 details" })).getByRole("button", { name: "More" }));
    // A press in the menu, drawn outside the sheet, is not one that closes it.
    fireEvent.pointerDown(screen.getByRole("menuitem", { name: "Remove" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "remove", id: "T2" } } });
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  test("a due day is set in the card sheet, and marked on an open card once it has come", async () => {
    stored = [
      { id: "T1", title: "Late", due: "2000-01-01", column: "todo" },
      { id: "T2", title: "Now", due: localDay(), column: "doing" },
      { id: "T3", title: "Later", due: "2999-01-01", column: "todo" },
      { id: "T4", title: "Finished late", due: "2000-01-01", column: "done" },
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
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    expect((sheet.getByLabelText("Due") as HTMLInputElement).value).toBe("2000-01-01");
    expect(sheet.getByText((_, el) => el?.classList.contains("board-sheet-due-when") ?? false).className).toContain("past");
    fireEvent.change(sheet.getByLabelText("Due"), { target: { value: "2999-06-01" } });
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "edit", id: "T1", due: "2999-06-01" } } });
    expect(sheet.getByText((_, el) => el?.classList.contains("board-sheet-due-when") ?? false).className).toContain("ahead");
    expect(due("Late").className).toContain("ahead");

    // A finished card's day is not marked.
    fireEvent.click(column("Done").getByText("Finished late"));
    const done = within(screen.getByRole("complementary", { name: "T4 details" }));
    expect(done.getByText((_, el) => el?.classList.contains("board-sheet-due-when") ?? false).className).toContain("ahead");
  });

  test("a comment is written under the card, the newest first, counted on the card, and deleted", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    const box = sheet.getByLabelText("New comment") as HTMLTextAreaElement;
    fireEvent.focus(box);
    expect((sheet.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);

    const before = Date.now();
    fireEvent.change(box, { target: { value: "asked the bank" } });
    fireEvent.keyDown(box, { key: "Enter", code: "Enter", ...(IS_MAC ? { metaKey: true } : { ctrlKey: true }) });
    await settle();
    const sent = calls.at(-1)!.args!.change as { op: string; id: string; text: string; at: string };
    expect({ ...sent, at: undefined }).toEqual({ op: "comment", id: "T1", text: "asked the bank", at: undefined });
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
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "uncomment", id: "T1", comment: 1 } } });
    expect(sheet.queryByText("asked the bank")).toBeNull();
    expect(column("To do").getByTitle("1 comments")).toBeTruthy();
  });

  test("a comment can be dictated while voice input is on, and not otherwise", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const comments = () => within(screen.getByRole("region", { name: "Comments" }));
    expect(comments().queryByTitle("Dictate from")).toBeNull();
    cleanup();

    features = [{ id: "voice-input", on: true }];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    await settle();
    expect(comments().getByTitle("Dictate from")).toBeTruthy();
    expect(screen.getByRole("region", { name: "Comments" }).querySelector(".dictate-mic")).toBeTruthy();
  });

  test("a refused comment stays in its box", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    refusal = "the card has 100 comments — remove the ones no longer needed";
    fireEvent.change(sheet.getByLabelText("New comment"), { target: { value: "one more" } });
    fireEvent.click(sheet.getByRole("button", { name: "Save" }));
    await settle();
    expect(sheet.getByText(refusal)).toBeTruthy();
    expect((sheet.getByLabelText("New comment") as HTMLTextAreaElement).value).toBe("one more");
  });

  test("tags are added and taken off in the sheet, shown on the card, and marked as the user's alone", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("File the memory request"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    fireEvent.click(sheet.getByRole("button", { name: "More info" }));
    expect(sheet.getByRole("tooltip").textContent).toContain("the Assistant does not see a card's tags");

    const box = sheet.getByLabelText("New tag") as HTMLInputElement;
    for (const tag of ["work", " bank "]) {
      fireEvent.change(box, { target: { value: tag } });
      fireEvent.keyDown(box, { key: "Enter", code: "Enter" });
      await settle();
    }
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "tags", id: "T1", tags: ["work", "bank"] } } });
    expect(box.value).toBe("");
    const onCard = () => [...screen.getByRole("region", { name: "To do" }).querySelectorAll(".board-card-tag")].map((t) => t.textContent);
    expect(onCard()).toEqual(["work", "bank"]);

    fireEvent.click(sheet.getByRole("button", { name: "Remove the tag work" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "tags", id: "T1", tags: ["bank"] } } });
    expect(onCard()).toEqual(["bank"]);

    // A refused one stays in the box.
    refusal = "the tag is 31 characters; keep it under 30";
    fireEvent.change(box, { target: { value: "x".repeat(31) } });
    fireEvent.keyDown(box, { key: "Enter", code: "Enter" });
    await settle();
    expect(box.value).toBe("x".repeat(31));
    expect(sheet.getByText(refusal)).toBeTruthy();
  });

  test("the tag box offers the board's other tags as it is typed in, and creates a new one", async () => {
    stored = [
      { id: "T1", title: "Fix the build", column: "todo" },
      { id: "T2", title: "Renew the cert", tags: ["Kibo", "home"], column: "doing" },
    ];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("Fix the build"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
    const box = sheet.getByRole("combobox", { name: "New tag" }) as HTMLInputElement;
    const offered = () => sheet.queryAllByRole("option").map((o) => o.textContent);
    const active = () => sheet.queryAllByRole("option").find((o) => o.getAttribute("aria-selected") === "true")?.textContent;
    const type = (text: string) => fireEvent.change(box, { target: { value: text } });
    const press = (code: string) => fireEvent.keyDown(box, { key: code, code });

    // Focused, every tag the card lacks; none picked, so Enter adds nothing.
    fireEvent.focus(box);
    expect(offered()).toEqual(["home", "Kibo"]);
    press("Enter");
    expect(calls.some((c) => c.command === "board_change")).toBe(false);

    // Typed and wiped out again: nothing picked.
    type("ki");
    type("");
    press("Enter");
    expect(calls.some((c) => c.command === "board_change")).toBe(false);

    // Typed: those that hold it, then creating it; the first picked.
    type("ki");
    expect(offered()).toEqual(["Kibo", "Create “ki”"]);
    expect(active()).toBe("Kibo");
    press("Enter");
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "tags", id: "T1", tags: ["Kibo"] } } });
    expect(box.value).toBe("");

    // ↓ to Create, and Enter creates it; ↑ goes round.
    type("ho");
    press("ArrowUp");
    expect(active()).toBe("Create “ho”");
    press("ArrowDown");
    expect(active()).toBe("home");
    press("ArrowDown");
    press("Enter");
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "tags", id: "T1", tags: ["Kibo", "ho"] } } });

    // A tag the board has, in another case, is offered in its own spelling and not created.
    type("HOME");
    expect(offered()).toEqual(["home"]);
    fireEvent.click(sheet.getByRole("option", { name: "home" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "tags", id: "T1", tags: ["Kibo", "ho", "home"] } } });

    // Escape closes the list and leaves the sheet open.
    type("x");
    press("Escape");
    expect(offered()).toEqual([]);
    expect(screen.getByRole("complementary", { name: "T1 details" })).toBeTruthy();

    // A tag taken off the last card that had it leaves the board's list.
    const filter = () => within(screen.getByRole("group", { name: "Show cards tagged" })).getAllByRole("button").map((b) => b.textContent);
    expect(filter()).toEqual(["All", "ho", "home", "Kibo"]);
    fireEvent.click(sheet.getByRole("button", { name: "Remove the tag ho" }));
    await settle();
    expect(filter()).toEqual(["All", "home", "Kibo"]);
    type("");
    expect(offered()).toEqual([]);
  });

  test("the tags over the columns show only the cards with any picked, and All shows them all again", async () => {
    stored = [
      { id: "T1", title: "Fix the build", tags: ["Kibo"], column: "todo" },
      { id: "T2", title: "Renew the cert", tags: ["bank", "home"], column: "doing" },
      { id: "T3", title: "Untagged", column: "todo" },
      { id: "T4", title: "Old Kibo one", tags: ["Kibo"], column: "archived" },
      { id: "T5", title: "Old other one", column: "archived" },
    ];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    const bar = within(screen.getByRole("group", { name: "Show cards tagged" }));
    expect(bar.getAllByRole("button").map((b) => b.textContent)).toEqual(["All", "bank", "home", "Kibo"]);
    expect(bar.getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");
    const titles = () => [...document.querySelectorAll(".board-card-title")].map((t) => t.textContent);
    fireEvent.click(screen.getByRole("button", { name: /Archive/ }));

    fireEvent.click(bar.getByRole("button", { name: "Kibo" }));
    expect(titles()).toEqual(["Fix the build", "Old Kibo one"]);
    expect(bar.getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("false");
    expect(column("Doing").getByText("No cards with these tags")).toBeTruthy();

    fireEvent.click(bar.getByRole("button", { name: "home" }));
    expect(titles()).toEqual(["Fix the build", "Renew the cert", "Old Kibo one"]);
    fireEvent.click(bar.getByRole("button", { name: "Kibo" }));
    expect(titles()).toEqual(["Renew the cert"]);

    fireEvent.click(bar.getByRole("button", { name: "All" }));
    expect(titles()).toHaveLength(5);

    // A card added under the filter gets its tags, says so while written, and stays in sight.
    fireEvent.click(bar.getByRole("button", { name: "Kibo" }));
    fireEvent.click(column("To do").getByLabelText("Add a card"));
    const form = within(screen.getByRole("group", { name: "New card" }));
    expect(form.getByLabelText("Tags it gets").textContent).toBe("Kibo");
    fireEvent.change(form.getByLabelText("Title"), { target: { value: "Ship it" } });
    fireEvent.keyDown(form.getByLabelText("Title"), { key: "Enter", code: "Enter" });
    await settle();
    expect(calls.at(-1)).toEqual({ command: "board_change", args: { change: { op: "add", title: "Ship it", note: "", due: "", tags: ["Kibo"] } } });
    expect(column("To do").getByText("Ship it")).toBeTruthy();
    fireEvent.click(bar.getByRole("button", { name: "All" }));
    stored = stored.filter((c) => c.title !== "Ship it");
    act(() => listeners.forEach((handler) => handler()));
    await settle();

    // A picked tag no card has any longer is let go of.
    fireEvent.click(bar.getByRole("button", { name: "home" }));
    expect(titles()).toEqual(["Renew the cert"]);
    stored = stored.map((c) => (c.id === "T2" ? { ...c, tags: ["bank"] } : c));
    act(() => listeners.forEach((handler) => handler()));
    await settle();
    expect(titles()).toHaveLength(5);
    expect(bar.getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");
  });

  test("a board without tags has no filter over it", async () => {
    render(<BoardPage openPane={() => {}} />);
    await settle();
    expect(screen.queryByRole("group", { name: "Show cards tagged" })).toBeNull();
  });

  test("a description is read as Markdown, and a link in it is followed rather than edited", async () => {
    stored = [{ id: "T1", title: "Read", note: "See **the spec** at [docs](https://example.com)", column: "todo" }];
    render(<BoardPage openPane={() => {}} />);
    await settle();
    fireEvent.click(column("To do").getByText("Read"));
    const sheet = within(screen.getByRole("complementary", { name: "T1 details" }));
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
        { id: "T1", title: "Long", note: "one\ntwo\nthree\nfour", column: "todo" },
        { id: "T2", title: "Short", note: "one line", column: "todo" },
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
    stored = [...stored, { id: "T3", title: "Call the bank", column: "todo" }];
    act(() => listeners.forEach((handler) => handler()));
    await settle();
    expect(column("To do").getByText("Call the bank")).toBeTruthy();
  });

  test("a board with only an archive says how cards get there", async () => {
    stored = [{ id: "T3", title: "Old one", column: "archived" }];
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
  const order = { todo: ["T1", "T2", "T3"], doing: ["T4"], done: ["T5"] };

  test("is read top to bottom per open column, the archive left out", () => {
    const card = (id: string, column: Card["column"]): Card => ({ id, title: id, column });
    expect(orderOf([card("T1", "done"), card("T2", "todo"), card("T3", "archived"), card("T4", "todo")])).toEqual({ todo: ["T2", "T4"], doing: [], done: ["T1"] });
  });

  test("among the cards a filter shows, goes just above the shown one it was dropped above, or under the one above it", () => {
    // T2 and T4 are hidden: the shown To do is T1, T3, T5.
    const all = { todo: ["T1", "T2", "T3", "T4", "T5"], doing: ["T6", "T7"], done: ["T8"] };
    const shown = filterOrder(all, (id) => !["T2", "T4", "T7", "T8"].includes(id));
    expect(shown).toEqual({ todo: ["T1", "T3", "T5"], doing: ["T6"], done: [] });
    // T5 dropped above T3: just above it among all, past the hidden T2.
    expect(dropChange(shown, { ...shown, todo: ["T1", "T5", "T3"] }, "T5", all)).toEqual({ op: "move", id: "T5", column: "todo", index: 2 });
    // T1 dropped last: just under T5, the end.
    expect(dropChange(shown, { ...shown, todo: ["T3", "T5", "T1"] }, "T1", all)).toEqual({ op: "move", id: "T1", column: "todo", index: 4 });
    // Into Doing under T6: just under it, above the hidden T7.
    expect(dropChange(shown, { ...shown, todo: ["T1", "T5"], doing: ["T6", "T3"] }, "T3", all)).toEqual({ op: "move", id: "T3", column: "doing", index: 1 });
    // Into a column whose cards are all hidden: under them.
    expect(dropChange(shown, { ...shown, todo: ["T1", "T5"], done: ["T3"] }, "T3", all)).toEqual({ op: "move", id: "T3", column: "done", index: 1 });
  });

  test("tags are listed once each whatever their case, the first spelling kept, alphabetically", () => {
    const card = (tags?: string[]): Card => ({ id: "T1", title: "t", column: "todo", tags });
    expect(tagsOf([card(["kibo", "Bank"]), card(["bank"]), card(), card(["Alpha", "Kibo"])])).toEqual(["Alpha", "Bank", "kibo"]);
    expect(hasAnyTag(card(["bank"]), ["Bank"])).toBe(true);
    expect(hasAnyTag(card(["bank"]), ["home"])).toBe(false);
    expect(hasAnyTag(card(), ["bank"])).toBe(false);
  });

  test("moves to the column and place the drag left it in", () => {
    expect(dropChange(order, { ...order, todo: ["T2", "T3", "T1"] }, "T1")).toEqual({ op: "move", id: "T1", column: "todo", index: 2 });
    expect(dropChange(order, { ...order, todo: ["T1", "T3"], doing: ["T2", "T4"] }, "T2")).toEqual({ op: "move", id: "T2", column: "doing", index: 0 });
  });

  test("back where it started, or never in the order, sends nothing", () => {
    expect(dropChange(order, order, "T2")).toBeNull();
    expect(dropChange(order, { ...order, todo: ["T1", "T3"], doing: ["T4"] }, "T2")).toBeNull();
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

describe("the tag box's choices", () => {
  test("are the tags the card lacks that hold what was typed, then creating it unless the board has it", () => {
    const known = ["bank", "home", "Kibo"];
    expect(tagChoices("", known, ["home"])).toEqual([{ tag: "bank", create: false }, { tag: "Kibo", create: false }]);
    expect(tagChoices(" K ", known, [])).toEqual([{ tag: "bank", create: false }, { tag: "Kibo", create: false }, { tag: "K", create: true }]);
    expect(tagChoices("kibo", known, [])).toEqual([{ tag: "Kibo", create: false }]);
    // One the card already has is neither offered nor created again.
    expect(tagChoices("HOME", known, ["home"])).toEqual([]);
    expect(tagChoices("new", ["mine"], ["mine"])).toEqual([{ tag: "new", create: true }]);
  });
});
