import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { EditorView } from "@codemirror/view";
import { language } from "@codemirror/language";
import { SearchQuery, setSearchQuery } from "@codemirror/search";
import { IS_MAC } from "../lib/shortcuts";
import type { FileTarget } from "../lib/chat";

// Editing a file in the IDE layout's viewer: the edit kept as a draft by
// path, saved over the version it began from, and what happens when the
// file changes on disk under it or its tab is closed.

let disk: Record<string, string | null>;
let writes: { path: string; expected: string | null; content: string }[];
let opened: string[] = [];
// Set, a write lands only when it resolves: a slow disk, for typing during one.
let slowDisk: Promise<void> | null = null;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command === "file_view") {
      const text = disk[args!.path as string] ?? null;
      return Promise.resolve({ old: text, new: text, unviewable: null });
    }
    if (command === "file_write") {
      const { path, expected, content } = args as { path: string; expected: string | null; content: string };
      writes.push({ path, expected, content });
      const write = () => {
        // What is there already is saved, whatever it was made on.
        if (disk[path] === content) return { kind: "saved" };
        if (expected !== null && disk[path] !== expected) return { kind: "changedOnDisk" };
        disk[path] = content;
        return { kind: "saved" };
      };
      return slowDisk ? slowDisk.then(write) : Promise.resolve(write());
    }
    if (command === "git_changes") return Promise.resolve({ staged: [], unstaged: [] });
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
const listeners = new Map<string, Set<(message: { payload: unknown }) => void>>();
const emit = (channel: string, payload: unknown) => listeners.get(channel)?.forEach((handler) => handler({ payload }));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (!listeners.has(channel)) listeners.set(channel, new Set());
    listeners.get(channel)!.add(handler);
    return Promise.resolve(() => listeners.get(channel)!.delete(handler));
  },
}));
mock.module("@tauri-apps/plugin-opener", () => ({
  openUrl: (url: string) => {
    opened.push(url);
    return Promise.resolve();
  },
}));

// The diagram engine is seven megabytes of browser code: here, its source back as an SVG.
const drawn: string[] = [];
mock.module("../lib/plantuml", () => ({
  plantumlSvg: (source: string) => {
    drawn.push(source);
    return source.includes("broken") ? Promise.reject(new Error("Syntax error")) : Promise.resolve(`<svg>${source}</svg>`);
  },
}));

// The window's close button, ⌘W or ⌘Q: whether the window stayed open.
let closeRequested: ((event: { preventDefault: () => void }) => Promise<void>) | null = null;
mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: (handler: typeof closeRequested) => {
      closeRequested = handler;
      return Promise.resolve(() => void (closeRequested = null));
    },
  }),
}));
async function closeWindow() {
  let kept = false;
  await act(() => closeRequested!({ preventDefault: () => void (kept = true) }));
  return kept;
}

/** The agent wrote a file: the folder's watcher says so, and the viewer reads again. */
const agentWrites = (path: string, text: string) => {
  disk[path] = text;
  act(() => emit("workspace-index:event", { root: "/repo", kind: "syncStarted" }));
};

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { FileViewer } = await import("../components/FileViewer");
const { useOpenFiles } = await import("../hooks/useOpenFiles");
const settle = (ms = 0) => act(() => new Promise((resolve) => setTimeout(resolve, ms)));
const MOD = IS_MAC ? { metaKey: true } : { ctrlKey: true };

const doc: FileTarget = { path: "docs/a.adoc", side: "worktree" };
const other: FileTarget = { path: "docs/b.adoc", side: "worktree" };

/** The viewer as the IDE layout has it, over the real open files and drafts. */
function Ide({ open }: { open: FileTarget[] }) {
  const viewer = useOpenFiles("/repo");
  useEffect(() => open.forEach((file) => viewer.open(file, true)), []);
  if (!viewer.active)
    return (
      <button type="button" data-testid="closed" onClick={() => viewer.open(open[0], true)}>
        Open again
      </button>
    );
  return (
    <FileViewer
      files={viewer.files}
      active={viewer.active}
      preview={viewer.preview}
      workspace="/repo"
      onActivate={viewer.open}
      onPin={viewer.pin}
      onClose={viewer.close}
      onCloseAll={viewer.closeAll}
      wrap
      compact
      edits={viewer.edits}
    />
  );
}

async function ide(open: FileTarget[] = [doc]) {
  render(<Ide open={open} />);
  // A document opens as it renders: to the file's text, to edit it — which
  // the viewer keeps for the tabs after. Then the editor's lazy load.
  await toEdit();
}
async function toEdit() {
  await settle(10);
  const file = screen.queryByRole("tab", { name: "File" });
  if (file) fireEvent.click(file);
  for (let i = 0; i < 5 && !document.querySelector(".cm-editor"); i++) await settle(10);
}
const editor = () => EditorView.findFromDOM(document.querySelector(".cm-editor") as HTMLElement)!;
const shown = () => editor().state.doc.toString();
/** What the editor shows once the tab's file is read and its editor drawn. */
async function showing(text: string) {
  for (let i = 0; i < 20; i++) {
    if (document.querySelector(".cm-editor") && shown() === text) return;
    await settle(5);
  }
  expect(document.querySelector(".cm-editor") && shown()).toBe(text);
}
/** A tab by its file's name; one with an edit is also named "Not saved". */
const tab = (name: string) => screen.getByRole("tab", { name: new RegExp(`^${name.replace(".", "\\.")}`) });
/** Typed, as far as the editor can tell; then the pause that passes it up. */
async function type(text: string) {
  act(() => editor().dispatch({ changes: { from: editor().state.doc.length, insert: text } }));
  await settle(300);
}
const save = () => act(() => void fireEvent.keyDown(editor().contentDOM, { code: "KeyS", ...MOD }));
const unsaved = () => document.querySelectorAll(".file-tab-unsaved").length;

beforeEach(() => {
  disk = { "docs/a.adoc": "= Title\n", "docs/b.adoc": "= B\n" };
  writes = [];
  slowDisk = null;
});

describe("editing in the IDE layout", () => {
  test("a file opens in the editor; an edit marks its tab and saving writes it over what was opened", async () => {
    await ide();
    expect(shown()).toBe("= Title\n");
    expect(unsaved()).toBe(0);
    await type("More.\n");
    expect(unsaved()).toBe(1);
    save();
    await settle(10);
    expect(writes).toEqual([{ path: "docs/a.adoc", expected: "= Title\n", content: "= Title\nMore.\n" }]);
    expect(unsaved()).toBe(0);
    expect(shown()).toBe("= Title\nMore.\n");
  });

  test("typing on while a save is being written keeps what was typed, as an edit still to save", async () => {
    await ide();
    await type("Mine.\n");
    let land = () => {};
    slowDisk = new Promise((resolve) => (land = resolve));
    save();
    await type("More.");
    slowDisk = null;
    land();
    await settle(10);
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\n");
    expect(shown()).toBe("= Title\nMine.\nMore.");
    expect(unsaved()).toBe(1);
    expect(screen.queryByRole("alert")).toBeNull();
    save();
    await settle(10);
    expect(writes.at(-1)).toEqual({ path: "docs/a.adoc", expected: "= Title\nMine.\n", content: "= Title\nMine.\nMore." });
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\nMore.");
    expect(unsaved()).toBe(0);
  });

  test("typed back to what is on disk, it is not an edit, and saving writes nothing", async () => {
    await ide();
    await type("x");
    act(() => editor().dispatch({ changes: { from: editor().state.doc.length - 1, to: editor().state.doc.length } }));
    await settle(300);
    expect(unsaved()).toBe(0);
    save();
    await settle(10);
    expect(writes).toEqual([]);
  });

  test("the file changed on disk: nothing is written, and the edit can give way or be kept", async () => {
    await ide();
    await type("Mine.\n");
    disk["docs/a.adoc"] = "= Title\nThe agent's.\n";
    save();
    await settle(10);
    expect(writes.at(-1)?.expected).toBe("= Title\n");
    expect(disk["docs/a.adoc"]).toBe("= Title\nThe agent's.\n");
    expect(screen.getByRole("alert").textContent).toContain("Changed on disk");
    expect(shown()).toBe("= Title\nMine.\n");

    // Kept: the next save writes over the agent's version, knowingly.
    fireEvent.click(screen.getByText("Keep mine"));
    expect(screen.queryByRole("alert")).toBeNull();
    save();
    await settle(10);
    expect(writes.at(-1)).toEqual({ path: "docs/a.adoc", expected: "= Title\nThe agent's.\n", content: "= Title\nMine.\n" });
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\n");
  });

  test("loading from disk drops the edit and shows the file as it is now", async () => {
    await ide();
    await type("Mine.\n");
    disk["docs/a.adoc"] = "= Title\nThe agent's.\n";
    save();
    await settle(10);
    fireEvent.click(screen.getByText("Load from disk"));
    await showing("= Title\nThe agent's.\n");
    expect(unsaved()).toBe(0);
  });

  test("an edit that cannot be saved outlives its tab being switched away from", async () => {
    await ide([doc, other]);
    fireEvent.click(tab("a.adoc"));
    await showing("= Title\n");
    await type("Draft.\n");
    disk["docs/a.adoc"] = "= Title\nThe agent's.\n";
    fireEvent.click(tab("b.adoc"));
    await showing("= B\n");
    expect(disk["docs/a.adoc"]).toBe("= Title\nThe agent's.\n");
    expect(unsaved()).toBe(1);
    fireEvent.click(tab("a.adoc"));
    await showing("= Title\nDraft.\n");
    expect(screen.getByRole("alert").textContent).toContain("Changed on disk");
  });

  test("closing a tab with an edit saves it first, and a tab that cannot be saved stays", async () => {
    await ide();
    await type("Mine.\n");
    fireEvent.click(screen.getByTitle("Close"));
    await settle(10);
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\n");
    expect(screen.getByTestId("closed")).toBeTruthy();
    fireEvent.click(screen.getByText("Open again"));
    await toEdit();
    await showing("= Title\nMine.\n");
    expect(unsaved()).toBe(0);

    // Changed on disk under the edit: not written, not closed — the banner asks.
    await type("More.\n");
    disk["docs/a.adoc"] = "= Title\nThe agent's.\n";
    fireEvent.click(screen.getByTitle("Close"));
    await settle(10);
    expect(disk["docs/a.adoc"]).toBe("= Title\nThe agent's.\n");
    expect(tab("a.adoc")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toContain("Changed on disk");
    // While the banner asks, nothing tries again: closing once more writes nothing.
    const tries = writes.length;
    fireEvent.click(screen.getByTitle("Close"));
    await settle(10);
    expect(writes.length).toBe(tries);
    expect(tab("a.adoc")).toBeTruthy();
  });

  test("a text saved before is saved again once something else was written in between", async () => {
    await ide();
    const blur = () => act(() => void fireEvent.focusOut(editor().contentDOM, { relatedTarget: null }));
    await type("Mine.\n");
    blur();
    await settle(10);
    agentWrites("docs/a.adoc", "= Title\nThe agent's.\n");
    await showing("= Title\nThe agent's.\n");
    act(() => editor().dispatch({ changes: { from: 0, to: editor().state.doc.length, insert: "= Title\nMine.\n" } }));
    await settle(300);
    blur();
    await settle(10);
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\n");
    expect(writes.at(-1)?.expected).toBe("= Title\nThe agent's.\n");
  });

  test("losing focus saves the edit, once, and only when there is one", async () => {
    await ide();
    const blur = () => act(() => void fireEvent.focusOut(editor().contentDOM, { relatedTarget: null }));
    blur();
    await settle(10);
    expect(writes).toEqual([]);

    await type("Mine.\n");
    blur();
    await settle(10);
    blur();
    await settle(10);
    expect(writes).toEqual([{ path: "docs/a.adoc", expected: "= Title\n", content: "= Title\nMine.\n" }]);
    expect(unsaved()).toBe(0);
  });

  test("switching to another tab saves the one left", async () => {
    await ide([doc, other]);
    await type("B, edited.\n");
    fireEvent.click(tab("a.adoc"));
    await showing("= Title\n");
    expect(disk["docs/b.adoc"]).toBe("= B\nB, edited.\n");
    expect(unsaved()).toBe(0);
  });

  test("a file of another language is highlighted once its language loads", async () => {
    disk["src/Main.java"] = "class Main {}\n";
    await ide([{ path: "src/Main.java", side: "worktree" }]);
    for (let i = 0; i < 50 && !editor().state.facet(language); i++) await settle(10);
    expect(editor().state.facet(language)?.name).toBe("java");
  });

  test("closing the window saves what is being typed, past the draft passed up before it", async () => {
    await ide();
    await type("Mine.\n");
    act(() => editor().focus());
    act(() => editor().dispatch({ changes: { from: editor().state.doc.length, insert: "More." } }));
    expect(await closeWindow()).toBe(false);
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\nMore.");
  });

  test("closing the window saves another tab's edit that can be saved by now", async () => {
    await ide([doc, other]);
    fireEvent.click(tab("a.adoc"));
    await showing("= Title\n");
    await type("Draft.\n");
    disk["docs/a.adoc"] = "= Title\nThe agent's.\n";
    fireEvent.click(tab("b.adoc"));
    await showing("= B\n");
    await type("Mine.\n");
    act(() => editor().focus());
    // Put back as the edit began from.
    disk["docs/a.adoc"] = "= Title\n";
    expect(await closeWindow()).toBe(false);
    expect(disk["docs/a.adoc"]).toBe("= Title\nDraft.\n");
    expect(disk["docs/b.adoc"]).toBe("= B\nMine.\n");
  });

  test("closing the window saves another tab's edit; one that cannot be saved keeps it open, once", async () => {
    await ide([doc, other]);
    fireEvent.click(tab("a.adoc"));
    await showing("= Title\n");
    await type("Draft.\n");
    disk["docs/a.adoc"] = "= Title\nThe agent's.\n";
    fireEvent.click(tab("b.adoc"));
    await showing("= B\n");
    await type("Mine.\n");
    act(() => editor().focus());

    expect(await closeWindow()).toBe(true);
    expect(disk["docs/b.adoc"]).toBe("= B\nMine.\n");
    expect(disk["docs/a.adoc"]).toBe("= Title\nThe agent's.\n");
    // Its tab comes up, saying why, and what closing again does.
    await showing("= Title\nDraft.\n");
    expect(screen.getByRole("alert").textContent).toContain("Changed on disk");
    expect(screen.getByRole("alert").textContent).toContain("Close the window again");
    expect(await closeWindow()).toBe(false);
  });

  test("closing the window with nothing typed writes nothing", async () => {
    await ide([doc, other]);
    expect(await closeWindow()).toBe(false);
    expect(writes).toEqual([]);
  });

  test("⌘F opens the search in the file, and Escape closes it", async () => {
    await ide();
    act(() => void fireEvent.keyDown(editor().contentDOM, { code: "KeyF", key: "f", ...MOD }));
    const field = document.querySelector<HTMLInputElement>(".cm-search input[name=search]");
    expect(field).toBeTruthy();
    act(() => void fireEvent.keyDown(field!, { code: "Escape", key: "Escape" }));
    expect(document.querySelector(".cm-search")).toBeNull();
  });

  test("⌘G and ⇧⌘G step through the matches", async () => {
    await ide();
    await type("Title. Title.");
    const at = () => editor().state.selection.main.from;
    act(() => editor().dispatch({ selection: { anchor: 0 }, effects: setSearchQuery.of(new SearchQuery({ search: "Title" })) }));
    const key = (shift: boolean) => act(() => void fireEvent.keyDown(editor().contentDOM, { code: "KeyG", key: "g", shiftKey: shift, ...MOD }));
    key(false);
    expect(at()).toBe(2);
    key(false);
    expect(at()).toBe(8);
    key(true);
    expect(at()).toBe(2);
  });

  test("an AsciiDoc file opens as it renders, its includes put in and its HTML made safe", async () => {
    disk["docs/a.adoc"] = [
      "= Title",
      "",
      "include::parts/one.adoc[]",
      "",
      "https://example.com[out] and <<_two,in>>",
      "",
      "++++",
      '<script>window.hacked = 1</script><img src="x" onerror="window.hacked = 1"><p id="root" style="position: fixed">raw</p>',
      "++++",
      "",
    ].join("\n");
    disk["docs/parts/one.adoc"] = "== One\n\ninclude::two.adoc[]\n";
    disk["docs/parts/two.adoc"] = "== Two\n\nIncluded twice over.\n";
    render(<Ide open={[doc]} />);
    // The converter loads the first time; cold, in a full run, that takes a while.
    for (let i = 0; i < 500 && !document.querySelector(".adoc p"); i++) await settle(10);
    const page = document.querySelector(".adoc")!;
    expect([...page.querySelectorAll("h1, h2")].map((h) => h.textContent)).toEqual(["Title", "One", "Two"]);
    expect(page.textContent).toContain("Included twice over.");
    // The classes the look is drawn by stay.
    expect(page.querySelectorAll(".sect1").length).toBe(2);
    expect(page.querySelector("script")).toBeNull();
    expect(page.querySelector("img")?.getAttribute("onerror")).toBeNull();
    const raw = [...page.querySelectorAll("p")].find((p) => p.textContent === "raw")!;
    expect(raw.getAttribute("style")).toBeNull();
    expect(raw.id).toBe("user-content-root");

    let scrolled: Element | null = null;
    const scroll = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled = this;
    };
    fireEvent.click(screen.getByText("in"));
    Element.prototype.scrollIntoView = scroll;
    expect((scrolled as Element | null)?.textContent).toBe("Two");
    opened = [];
    fireEvent.click(screen.getByText("out"));
    await settle(10);
    expect(opened).toEqual(["https://example.com"]);
  });

  test("PlantUML in an AsciiDoc file is drawn: a block, its include, a macro; one that fails shows its source", async () => {
    disk["docs/a.adoc"] = [
      "[plantuml]",
      "----",
      "A -> B",
      "----",
      "",
      "[plantuml]",
      "....",
      "include::flows/c.puml[]",
      "....",
      "",
      "plantuml::flows/d.puml[]",
      "",
      "[plantuml]",
      "----",
      "broken ->",
      "----",
      "",
      "plantuml::flows/none.puml[]",
      "",
    ].join("\n");
    disk["docs/flows/c.puml"] = "C -> D\n";
    disk["docs/flows/d.puml"] = "@startuml\nE -> F\n@enduml\n";
    drawn.length = 0;
    render(<Ide open={[doc]} />);
    for (let i = 0; i < 500 && document.querySelectorAll(".adoc img").length < 3; i++) await settle(10);
    expect(drawn).toEqual(["A -> B", "C -> D", "@startuml\nE -> F\n@enduml", "broken ->"]);
    const images = [...document.querySelectorAll<HTMLImageElement>(".adoc .adoc-diagram img")];
    expect(images.map((img) => decodeURIComponent(img.src.split(",")[1]))).toEqual([
      "<svg>A -> B</svg>",
      "<svg>C -> D</svg>",
      "<svg>@startuml\nE -> F\n@enduml</svg>",
    ]);
    const page = document.querySelector(".adoc")!.textContent;
    expect(page).toContain("Could not draw the diagram: Error: Syntax error");
    expect(page).toContain("broken ->");
    expect(page).toContain("Unresolved diagram: flows/none.puml");
  });

  test("a PlantUML file opens drawn, its text a tab away", async () => {
    disk["docs/flow.puml"] = "@startuml\nA -> B\n@enduml\n";
    render(<Ide open={[{ path: "docs/flow.puml", side: "worktree" }]} />);
    for (let i = 0; i < 100 && !document.querySelector(".adoc img"); i++) await settle(10);
    const img = document.querySelector<HTMLImageElement>(".adoc .adoc-diagram img")!;
    expect(decodeURIComponent(img.src.split(",")[1])).toBe("<svg>@startuml\nA -> B\n@enduml</svg>");
    // A click opens it the whole window over, fitted, to zoom.
    fireEvent.click(screen.getByTitle("Open larger"));
    const dialog = screen.getByRole("dialog", { name: "Diagram" });
    expect(dialog.querySelector<HTMLImageElement>(".diagram-view img")?.src).toBe(img.src);
    const level = () => dialog.querySelector(".diagram-zoom-level")?.textContent;
    expect(level()).toBe("Fit");
    fireEvent.click(screen.getByLabelText("Zoom in"));
    expect(level()).toBe("125%");
    fireEvent.click(screen.getByLabelText("Zoom out"));
    fireEvent.click(screen.getByLabelText("Zoom out"));
    expect(level()).toBe("80%");
    fireEvent.click(screen.getByText("Fit"));
    expect(level()).toBe("Fit");
    fireEvent.click(screen.getByText("100%"));
    expect(level()).toBe("100%");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();

    await toEdit();
    await showing("@startuml\nA -> B\n@enduml\n");
  });

  test("a source listing in an AsciiDoc file is coloured by its language; one without stays as written", async () => {
    disk["docs/a.adoc"] = "[source,java]\n----\nclass Order {}\nclass Line {}\n----\n\n----\nplain text\n----\n";
    render(<Ide open={[doc]} />);
    for (let i = 0; i < 300 && !document.querySelector(".adoc pre"); i++) await settle(10);
    // Then the grammar's colours, well inside the test's own time limit.
    for (let i = 0; i < 100 && !document.querySelector(".adoc pre code span"); i++) await settle(10);
    const [java, plain] = [...document.querySelectorAll(".adoc pre")];
    const code = java.querySelector("code")!;
    expect(code.className).toBe("language-java");
    expect(code.textContent).toBe("class Order {}\nclass Line {}");
    expect([...code.querySelectorAll("span")].find((span) => span.textContent === "class")?.getAttribute("style")).toContain("--shiki-dark");
    expect(plain.textContent).toBe("plain text");
    expect(plain.querySelector("span")).toBeNull();
  });

  test("a pause in typing saves it", async () => {
    await ide();
    await type("Mine.\n");
    expect(writes).toEqual([]);
    await settle(3000);
    expect(disk["docs/a.adoc"]).toBe("= Title\nMine.\n");
    expect(unsaved()).toBe(0);
  });

  test("the agent writing the file while nothing is typed shows its version", async () => {
    await ide();
    agentWrites("docs/a.adoc", "= Title\nThe agent's.\n");
    await showing("= Title\nThe agent's.\n");
    expect(unsaved()).toBe(0);
    // An edit of that version is made on it: it saves without a conflict.
    await type("Mine.\n");
    save();
    await settle(10);
    expect(writes).toEqual([{ path: "docs/a.adoc", expected: "= Title\nThe agent's.\n", content: "= Title\nThe agent's.\nMine.\n" }]);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  /// Typed and not yet passed up, the edit is only in the editor: a re-read
  /// landing then must not put the disk's text over it.
  test("the agent writing the file mid-typing does not take what was just typed", async () => {
    await ide();
    act(() => editor().dispatch({ changes: { from: editor().state.doc.length, insert: "Mine." } }));
    agentWrites("docs/a.adoc", "= Title\nThe agent's.\n");
    await settle(10);
    expect(shown()).toBe("= Title\nMine.");
    await settle(300);
    expect(unsaved()).toBe(1);
    expect(screen.getByRole("alert").textContent).toContain("Changed on disk");
  });

  test("saved straight after typing, before the edit is passed up, over a file the agent wrote meanwhile", async () => {
    await ide();
    act(() => editor().dispatch({ changes: { from: editor().state.doc.length, insert: "Mine." } }));
    agentWrites("docs/a.adoc", "= Title\nThe agent's.\n");
    await settle(10);
    save();
    await settle(10);
    expect(writes).toEqual([{ path: "docs/a.adoc", expected: "= Title\n", content: "= Title\nMine." }]);
    expect(disk["docs/a.adoc"]).toBe("= Title\nThe agent's.\n");
    expect(screen.getByRole("alert").textContent).toContain("Changed on disk");
  });
});
