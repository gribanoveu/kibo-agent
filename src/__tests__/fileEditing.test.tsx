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

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command === "file_view") {
      const text = disk[args!.path as string] ?? null;
      return Promise.resolve({ old: text, new: text, unviewable: null });
    }
    if (command === "file_write") {
      const { path, expected, content } = args as { path: string; expected: string | null; content: string };
      writes.push({ path, expected, content });
      // What is there already is saved, whatever it was made on.
      if (disk[path] === content) return Promise.resolve({ kind: "saved" });
      if (expected !== null && disk[path] !== expected) return Promise.resolve({ kind: "changedOnDisk" });
      disk[path] = content;
      return Promise.resolve({ kind: "saved" });
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
  // The file's read, then the editor's lazy load.
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
