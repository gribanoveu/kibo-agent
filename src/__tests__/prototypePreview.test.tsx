import { afterAll, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { emptyTurn, type Block } from "../lib/chatTurnReducer";

// A Prototype chat's Preview panel (docs/31-prototype-role.md, P-5): what was
// presented is read off the transcript, and the panel follows the turn.

const revealed: string[] = [];
const exported: unknown[] = [];
let exportFails = false;
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: { chatId: string }) => {
    if (command === "prototype_reveal") revealed.push(args.chatId);
    if (command === "prototype_export") {
      if (exportFails) return Promise.reject("no space left");
      exported.push(args);
    }
    return Promise.resolve(null);
  },
}));
// The save dialog answers with this; `null` is a cancel.
let savePath: string | null = null;
const saveAsked: unknown[] = [];
mock.module("@tauri-apps/plugin-dialog", () => ({
  open: () => Promise.resolve(null),
  save: (options: unknown) => {
    saveAsked.push(options);
    return Promise.resolve(savePath);
  },
}));
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { edits, pageUrl, pointedOf, pointNote, presentedOf, restoreNote, versionsOf } = await import("../lib/prototype");
const { usePrototypePreview } = await import("../hooks/usePrototypePreview");
const { PrototypePreview } = await import("../components/PrototypePreview");
const { Transcript } = await import("../components/ChatPanel");

type Tool = Extract<Block, { kind: "tool" }>;

const tool = (id: string, name: string, over: Partial<Tool> = {}): Tool => ({
  kind: "tool",
  id,
  round: 1,
  name,
  arguments: "{}",
  status: "done",
  output: "",
  ...over,
});

const presented = (id: string, version: number, path = "index.html", title = "Coffee shop") =>
  tool(id, "present", { result: { result: "presented", path, title, version } });

const edit = (id: string) => tool(id, "editFile");

describe("what was presented is the transcript's", () => {
  test("a settled present is a version; a refused one or another call is not", () => {
    expect(presentedOf(presented("a", 2))).toEqual({ version: 2, path: "index.html", title: "Coffee shop" });
    expect(presentedOf(tool("b", "present", { status: "failed" }))).toBeNull();
    expect(presentedOf(tool("c", "writeFile", { result: { result: "presented", version: 1, path: "x" } }))).toBeNull();
    const blocks: Block[] = [presented("a", 1), edit("e1"), presented("b", 2, "deck.html", "Deck")];
    expect(versionsOf(blocks).map((v) => v.version)).toEqual([1, 2]);
  });

  test("only finished changes to the folder count as edits", () => {
    expect(
      edits([edit("a"), tool("b", "writeFile"), tool("c", "delete"), tool("d", "readFile"), edit("e"), tool("f", "editFile", { status: "running" })]),
    ).toBe(4);
  });

  test("a page's address is the platform's, its path segments encoded", () => {
    expect(pageUrl("c1", null, "my page.html", false)).toBe("artifact://localhost/c1/files/my%20page.html");
    expect(pageUrl("c1", 3, "pages/index.html", true)).toBe("http://artifact.localhost/c1/versions/3/pages/index.html");
  });
});

describe("an element pointed at", () => {
  const picked = { kibo: "picked", selector: " main > h1 ", html: "<h1>Coffee</h1>", text: " Coffee " };

  test("is taken from the pointer's message alone, checked and cut to size", () => {
    expect(pointedOf(picked, "index.html")).toEqual({ page: "index.html", selector: "main > h1", html: "<h1>Coffee</h1>", text: "Coffee" });
    expect(pointedOf({ ...picked, kibo: "page-error" }, "index.html")).toBeNull();
    expect(pointedOf({ ...picked, html: 1 }, "index.html")).toBeNull();
    expect(pointedOf({ ...picked, selector: "  " }, "index.html")).toBeNull();
    expect(pointedOf(null, "index.html")).toBeNull();
    const long = pointedOf({ ...picked, html: "x".repeat(900), selector: "s".repeat(400), text: "t".repeat(300) }, "a.html")!;
    expect([long.html.length, long.html.endsWith("…"), long.selector.length, long.text.length]).toEqual([801, true, 300, 200]);
    expect(pointedOf({ ...picked, html: "x".repeat(800) }, "a.html")!.html).toBe("x".repeat(800));
  });

  test("tells the model which element, as drawn, and to change it alone", () => {
    const note = pointNote(pointedOf(picked, "index.html")!);
    expect(note).toStartWith("[The user pointed at an element on the page index.html: `main > h1`.");
    expect(note).toContain("```html\n<h1>Coffee</h1>\n```");
    expect(note).toContain("with editFile — do not write the page again");
  });
});

describe("a restore is a version too", () => {
  const restored = (id: string, from: number, version: number, before: number | null): Block => ({ kind: "restored", id, from, version, before });
  const user = (id: string): Block => ({ kind: "user", id, text: "hi" });

  test("the version put back is listed with the page it was, and the folder before it when it was kept", () => {
    const blocks: Block[] = [presented("p1", 1, "index.html"), presented("p2", 2, "deck.html", "Deck"), restored("r1", 1, 4, 3)];
    expect(versionsOf(blocks).slice(2)).toEqual([
      { version: 3, path: "deck.html", title: "Before restoring version 1" },
      { version: 4, path: "index.html", title: "Version 1, restored" },
    ]);
    expect(versionsOf([presented("p1", 1), restored("r1", 1, 2, null)]).map((v) => v.version)).toEqual([1, 2]);
  });

  test("the model is told of a restore since the user's last message, and of the last one only", () => {
    expect(restoreNote([user("u1"), presented("p1", 1)])).toBeNull();
    expect(restoreNote([user("u1"), restored("r1", 1, 3, null), user("u2")])).toBeNull();
    const note = restoreNote([user("u1"), restored("r1", 1, 3, null), restored("r2", 2, 4, null)]);
    expect(note).toContain("version 2 (now kept as version 4)");
    expect(note).toContain("read them before you edit them");
  });
});

describe("the panel follows the turn", () => {
  const hook = (initial: { chatId: string | null; blocks: Block[] | null; running: boolean }) =>
    renderHook((props: typeof initial) => usePrototypePreview(props.chatId, props.blocks, props.running), { initialProps: initial });

  test("a page presented in the turn under way opens the panel on the folder as it is", () => {
    const { result, rerender } = hook({ chatId: "c1", blocks: [], running: true });
    expect(result.current.open).toBe(false);
    rerender({ chatId: "c1", blocks: [presented("p1", 1)], running: true });
    expect(result.current.open).toBe(true);
    expect(result.current.shown).toBeNull();
    expect(result.current.url).toBe("artifact://localhost/c1/files/index.html");
  });

  test("a saved chat opens closed, its versions there to pick", () => {
    const { result, rerender } = hook({ chatId: "c1", blocks: [], running: false });
    // The chat is switched to first; its transcript arrives a render later.
    rerender({ chatId: "c2", blocks: [], running: false });
    rerender({ chatId: "c2", blocks: [presented("p1", 1), presented("p2", 2)], running: false });
    expect(result.current.open).toBe(false);
    expect(result.current.available).toBe(true);
    act(() => result.current.show(1));
    expect(result.current.open).toBe(true);
    expect(result.current.url).toBe("artifact://localhost/c2/versions/1/index.html");
  });

  test("an edit draws the folder again, but not a version kept", () => {
    const { result, rerender } = hook({ chatId: "c1", blocks: [presented("p1", 1)], running: false });
    act(() => result.current.show(null));
    const before = result.current.key;
    rerender({ chatId: "c1", blocks: [presented("p1", 1), edit("e1")], running: false });
    expect(result.current.key).not.toBe(before);

    act(() => result.current.show(1));
    const kept = result.current.key;
    rerender({ chatId: "c1", blocks: [presented("p1", 1), edit("e1"), edit("e2")], running: false });
    expect(result.current.key).toBe(kept);
    act(() => result.current.refresh());
    expect(result.current.key).not.toBe(kept);
  });

  test("expands over the chat until it is closed, toggled shut or another chat opens", () => {
    const { result, rerender } = renderHook(({ chatId }) => usePrototypePreview(chatId, [presented("p1", 1)], false), {
      initialProps: { chatId: "c1" },
    });
    act(() => result.current.show(null));
    act(() => result.current.toggleExpanded());
    expect(result.current.expanded).toBe(true);
    act(() => result.current.close());
    act(() => result.current.show(null));
    expect(result.current.expanded).toBe(false);
    act(() => result.current.toggleExpanded());
    act(() => result.current.toggle());
    act(() => result.current.toggle());
    expect(result.current.expanded).toBe(false);
    act(() => result.current.toggleExpanded());
    rerender({ chatId: "c2" });
    act(() => result.current.show(null));
    expect(result.current.expanded).toBe(false);
  });

  test("another chat closes it; a chat in another role has no panel", () => {
    const { result, rerender } = hook({ chatId: "c1", blocks: [presented("p1", 1)], running: false });
    act(() => result.current.show(null));
    rerender({ chatId: "c2", blocks: [presented("p9", 1)], running: false });
    expect(result.current.open).toBe(false);
    rerender({ chatId: "c2", blocks: null, running: false });
    expect(result.current.available).toBe(false);
  });
});

describe("the panel", () => {
  const panel = (blocks: Block[]) => {
    const { result } = renderHook(() => usePrototypePreview("c1", blocks, false));
    act(() => result.current.show(null));
    return result;
  };

  test("draws the page in a sandboxed frame that may not reach the window", () => {
    const preview = panel([presented("p1", 1)]);
    const { container } = render(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    const frame = container.querySelector("iframe")!;
    expect(frame.getAttribute("src")).toBe("artifact://localhost/c1/files/index.html");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts allow-modals allow-forms");
    expect(frame.getAttribute("allow")).toBe("fullscreen");
  });

  test("lists the page's own errors, ignores anyone else's, and hands one to the composer", () => {
    const preview = panel([presented("p1", 1)]);
    const asked: string[] = [];
    const shown = { ...preview.current, askFix: (text: string) => asked.push(text) };
    const { container } = render(<PrototypePreview preview={shown} chatId="c1" onNotify={() => {}} />);
    const page = container.querySelector("iframe")!.contentWindow;
    act(() => {
      window.dispatchEvent(new MessageEvent("message", { data: { kibo: "page-error", message: "x is not defined" }, source: page }));
      window.dispatchEvent(new MessageEvent("message", { data: { kibo: "page-error", message: "from elsewhere" }, source: window }));
      window.dispatchEvent(new MessageEvent("message", { data: { other: "thing" }, source: page }));
    });
    expect(screen.queryByText("x is not defined")).not.toBeNull();
    expect(screen.queryByText("from elsewhere")).toBeNull();
    fireEvent.click(screen.getByText("Fix"));
    expect(asked).toEqual(["The page shows an error: x is not defined. Fix it."]);
  });

  test("restores the version it shows, and offers nothing to restore on the folder as it is", () => {
    const preview = panel([presented("p1", 1), presented("p2", 2)]);
    const asked: number[] = [];
    const { rerender } = render(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} onRestore={(v) => asked.push(v)} />);
    expect(screen.queryByText("Restore")).toBeNull();
    act(() => preview.current.show(1));
    rerender(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} onRestore={(v) => asked.push(v)} />);
    fireEvent.click(screen.getByText("Restore"));
    expect(asked).toEqual([1]);
    // While a turn runs there is no restore to offer.
    rerender(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    expect((screen.getByText("Restore").closest("button") as HTMLButtonElement).disabled).toBe(true);
  });

  test("saves the page it shows as one file, where the user picked, and says so", async () => {
    const preview = panel([presented("p1", 1), presented("p2", 2, "deck.html", "Deck")]);
    act(() => preview.current.show(2));
    const told: string[] = [];
    render(<PrototypePreview preview={preview.current} chatId="c1" onNotify={(m) => told.push(m)} />);
    const save = screen.getByTitle("Save as one HTML file, for any browser");

    // A cancel saves nothing.
    await act(async () => fireEvent.click(save));
    expect(saveAsked).toEqual([{ defaultPath: "Deck.html", filters: [{ name: "HTML", extensions: ["html"] }] }]);
    expect(exported).toEqual([]);

    savePath = "/Users/me/Desktop/deck.html";
    await act(async () => fireEvent.click(save));
    expect(exported).toEqual([{ chatId: "c1", version: 2, page: "deck.html", path: "/Users/me/Desktop/deck.html" }]);
    expect(told).toEqual(["Saved to deck.html"]);

    exportFails = true;
    await act(async () => fireEvent.click(save));
    expect(told).toEqual(["Saved to deck.html", "no space left"]);
    exportFails = false;
    savePath = null;
  });

  test("saves the folder as it is when that is what it shows", async () => {
    const preview = panel([presented("p1", 1, "index.html", "Coffee")]);
    render(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    savePath = "/tmp/now.html";
    exported.length = 0;
    await act(async () => fireEvent.click(screen.getByTitle("Save as one HTML file, for any browser")));
    expect(exported).toEqual([{ chatId: "c1", version: null, page: "index.html", path: "/tmp/now.html" }]);
    savePath = null;
  });

  test("with no page left to show — a rewind took them all — it is not expanded", () => {
    const { result, rerender } = renderHook(({ blocks }) => usePrototypePreview("c1", blocks, false), {
      initialProps: { blocks: [presented("p1", 1)] as Block[] },
    });
    act(() => result.current.show(null));
    act(() => result.current.toggleExpanded());
    expect(result.current.expanded).toBe(true);
    rerender({ blocks: [] });
    expect(result.current.expanded).toBe(false);
  });

  test("its expand button says which way it goes", () => {
    const preview = panel([presented("p1", 1)]);
    const { rerender } = render(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    fireEvent.click(screen.getByTitle("Expand over the chat"));
    rerender(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    expect(screen.getByTitle("Back beside the chat").getAttribute("aria-pressed")).toBe("true");
  });

  test("points at an element only when asked, on the folder as it is, and hands it over", () => {
    const preview = panel([presented("p1", 1), presented("p2", 2)]);
    const pointed: unknown[] = [];
    const shown = { ...preview.current, point: (p: unknown) => pointed.push(p) };
    const { container, rerender } = render(<PrototypePreview preview={shown} chatId="c1" onNotify={() => {}} />);
    const page = container.querySelector("iframe")!.contentWindow!;
    const asked: unknown[] = [];
    page.postMessage = ((data: unknown) => asked.push(data)) as typeof page.postMessage;
    const say = (data: unknown) => act(() => window.dispatchEvent(new MessageEvent("message", { data, source: page })));
    const pick = { kibo: "picked", selector: "main > h1", html: "<h1>Hi</h1>", text: "Hi" };

    // Not asked: the page cannot put an element in the message box by itself.
    say(pick);
    expect(pointed).toEqual([]);

    fireEvent.click(screen.getByTitle("Point at an element to ask for a change to it alone"));
    expect(asked).toEqual([{ kibo: "pick" }]);
    expect(screen.getByTitle("Stop pointing (Escape on the page)").getAttribute("aria-pressed")).toBe("true");
    // Another window's word is not the page's.
    act(() => window.dispatchEvent(new MessageEvent("message", { data: pick, source: window })));
    expect(pointed).toEqual([]);
    say(pick);
    expect(pointed).toEqual([{ page: "index.html", selector: "main > h1", html: "<h1>Hi</h1>", text: "Hi" }]);
    // One pick per ask.
    say(pick);
    expect(pointed.length).toBe(1);

    // Asked again and stopped from the panel, or from the page.
    fireEvent.click(screen.getByTitle("Point at an element to ask for a change to it alone"));
    fireEvent.click(screen.getByTitle("Stop pointing (Escape on the page)"));
    expect(asked.at(-1)).toEqual({ kibo: "pick-stop" });
    fireEvent.click(screen.getByTitle("Point at an element to ask for a change to it alone"));
    say({ kibo: "pick-cancelled" });
    expect(screen.getByTitle("Point at an element to ask for a change to it alone").getAttribute("aria-pressed")).toBe("false");

    // A kept version is not pointed at: the folder may no longer have its elements.
    act(() => preview.current.show(1));
    rerender(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    expect(screen.queryByTitle("Point at an element to ask for a change to it alone")).toBeNull();
  });

  test("another chat drops the element pointed at", () => {
    const { result, rerender } = renderHook(({ chatId }) => usePrototypePreview(chatId, [presented("p1", 1)], false), {
      initialProps: { chatId: "c1" },
    });
    act(() => result.current.point({ page: "index.html", selector: "h1", html: "<h1/>", text: "" }));
    expect(result.current.pointed?.selector).toBe("h1");
    rerender({ chatId: "c2" });
    expect(result.current.pointed).toBeNull();
  });

  test("shows the chat's folder when asked", () => {
    const preview = panel([presented("p1", 1)]);
    render(<PrototypePreview preview={preview.current} chatId="c1" onNotify={() => {}} />);
    fireEvent.click(screen.getByTitle("Show the chat's folder"));
    expect(revealed).toEqual(["c1"]);
  });
});

describe("a restore in the transcript", () => {
  test("says which version came back and what it is now, and opens it", () => {
    const opened: number[] = [];
    const turn = { ...emptyTurn(), status: "done" as const, blocks: [presented("p1", 1), { kind: "restored", id: "r1", from: 1, version: 3, before: 2 } as Block] };
    render(<Transcript turn={turn} onDecide={() => {}} onOpenVersion={(v) => opened.push(v)} />);
    expect(screen.getByText("Restored version 1")).not.toBeNull();
    expect(screen.getByText("now version 3 · the folder before it kept as version 2")).not.toBeNull();
    fireEvent.click(screen.getAllByText("Open")[1]);
    expect(opened).toEqual([3]);
  });
});

describe("a presented page in the transcript", () => {
  test("stands on its own card, and its button opens that version", () => {
    const opened: number[] = [];
    const turn = { ...emptyTurn(), status: "done" as const, blocks: [edit("e1"), presented("p1", 2), edit("e2")] };
    render(<Transcript turn={turn} onDecide={() => {}} onOpenVersion={(v) => opened.push(v)} />);
    expect(screen.getByText("Coffee shop")).not.toBeNull();
    expect(screen.getByText("version 2 · index.html")).not.toBeNull();
    fireEvent.click(screen.getByText("Open"));
    expect(opened).toEqual([2]);
  });
  test("a message about an element shows its selector above the words typed, not the HTML sent", () => {
    const user: Block = { kind: "user", id: "u1", text: "make it bigger", sent: "make it bigger\n\n[… <h1>Coffee</h1> …]", element: "main > h1" };
    const turn = { ...emptyTurn(), status: "done" as const, blocks: [user] };
    const { container } = render(<Transcript turn={turn} onDecide={() => {}} />);
    expect(container.querySelector(".bubble-element code")?.textContent).toBe("main > h1");
    expect(container.querySelector(".bubble")?.textContent).toBe("make it bigger");
  });
});
