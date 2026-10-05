import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { FileTarget, FileView, WorkingChanges } from "../lib/chat";

// The viewer beside the chat: one file, its changes or the whole of it, read
// again when the folder changes.

let view: FileView;
let asked: unknown[] = [];
let changes: WorkingChanges;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command === "file_view") {
      asked.push(args);
      return Promise.resolve(structuredClone(view));
    }
    if (command === "git_changes") return Promise.resolve(structuredClone(changes));
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

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { FileViewer } = await import("../components/FileViewer");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

const ten = Array.from({ length: 10 }, (_, i) => `${i + 1}`).join("\n") + "\n";
beforeEach(() => {
  view = { old: ten, new: ten.replace("6\n", "six\n"), unviewable: null };
  asked = [];
  changes = { staged: [], unstaged: [] };
});

async function open(
  target: FileTarget = { path: "src/main.rs", side: "unstaged" },
  onCloseAll = () => {},
  wrap = true,
  compact = false,
) {
  const shown = render(
    <FileViewer
      files={[target]}
      active={target}
      preview={null}
      workspace="/repo"
      onActivate={() => {}}
      onPin={() => {}}
      onClose={() => {}}
      onCloseAll={onCloseAll}
      wrap={wrap}
      compact={compact}
    />,
  );
  await settle();
  return shown;
}
const rows = () => [...document.querySelectorAll(".diff-row")].map((row) => row.textContent);

describe("FileViewer", () => {
  test("asks for the file on its side and shows what changed, named and counted", async () => {
    await open();
    expect(asked).toEqual([{ path: "src/main.rs", side: "unstaged" }]);
    expect(document.querySelector(".file-viewer-title")?.textContent).toBe("src/main.rs");
    expect(screen.getByRole("tab", { name: "main.rs" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText("Unstaged")).toBeTruthy();
    expect(screen.getByText("+1")).toBeTruthy();
    expect(screen.getByText("-1")).toBeTruthy();
    expect(rows()).toHaveLength(9); // the hunk header, three lines each side, the line removed and added
    expect(document.querySelector(".diff-view")?.classList.contains("diff-view-plain")).toBe(false);
  });

  test("File shows every line with the change in place", async () => {
    await open();
    fireEvent.click(screen.getByRole("tab", { name: "File" }));
    expect(rows()).toHaveLength(11);
    expect(screen.getByRole("tab", { name: "File" }).getAttribute("aria-selected")).toBe("true");
  });

  test("a file with nothing changed is shown whole, and Diff is off", async () => {
    view = { old: "a\n", new: "a\n", unviewable: null };
    await open({ path: "a.txt", side: "worktree" });
    expect(rows()).toEqual(["11 a"]);
    // One number column, not the same number twice.
    expect(document.querySelector(".diff-view")?.classList.contains("diff-view-plain")).toBe(true);
    expect((screen.getByRole("tab", { name: "Diff" }) as HTMLButtonElement).disabled).toBe(true);
    // The open folder's files carry no side label.
    expect(document.querySelector(".file-viewer-side")).toBeNull();
  });

  test("says why a binary or huge file is not drawn", async () => {
    view = { old: null, new: null, unviewable: "binary" };
    await open();
    expect(screen.getByText("A binary file — not shown.")).toBeTruthy();
  });

  test("Escape closes it all", async () => {
    let closed = 0;
    await open(undefined, () => closed++);
    fireEvent.keyDown(document.querySelector(".file-viewer")!, { key: "Escape" });
    expect(closed).toBe(1);
  });

  test("reads again when the open folder changes, not another one", async () => {
    await open();
    view = { old: ten, new: ten.replace("2\n", "two\n"), unviewable: null };
    act(() => emit("workspace-index:event", { root: "/elsewhere", kind: "syncStarted" }));
    await settle();
    expect(asked).toHaveLength(1);
    act(() => emit("workspace-index:event", { root: "/repo", kind: "syncStarted" }));
    await settle();
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(asked).toHaveLength(3);
    expect(rows().some((row) => row?.includes("two"))).toBe(true);
  });

  test("a tab per open file: a click shows it, the cross or a middle click closes it", async () => {
    const a: FileTarget = { path: "src/a.rs", side: "unstaged" };
    const b: FileTarget = { path: "src/a.rs", side: "staged" };
    const c: FileTarget = { path: "README.md", side: "worktree" };
    const shown: FileTarget[] = [];
    const closed: FileTarget[] = [];
    const pinned: FileTarget[] = [];
    render(
      <FileViewer
        files={[a, b, c]}
        active={b}
        preview={c}
        workspace="/repo"
        onActivate={(t) => shown.push(t)}
        onPin={(t) => pinned.push(t)}
        onClose={(t) => closed.push(t)}
        onCloseAll={() => {}}
        wrap
      />,
    );
    await settle();
    // The same file from both sides says which is which; the other needs no letter.
    expect(screen.getAllByRole("tab").slice(0, 3).map((t) => t.textContent)).toEqual(["a.rsU", "a.rsS", "README.md"]);
    expect(asked).toEqual([{ path: "src/a.rs", side: "staged" }]);
    fireEvent.click(screen.getByRole("tab", { name: "README.md" }));
    expect(shown).toEqual([c]);
    // The preview tab is set apart, and a double click keeps it.
    const tabs = [...document.querySelectorAll(".file-tab")];
    expect(tabs.map((t) => t.classList.contains("preview"))).toEqual([false, false, true]);
    fireEvent.doubleClick(tabs[2]);
    expect(pinned).toEqual([c]);
    fireEvent.click(screen.getAllByTitle("Close")[0]);
    fireEvent(document.querySelectorAll(".file-tab")[2], new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(closed).toEqual([a, c]);
  });

  test("Markdown also reads as it renders, and unchanged opens that way", async () => {
    view = { old: "# Title\n\nSome *text*.\n", new: "# Title\n\nSome *text*.\n", unviewable: null };
    await open({ path: "docs/README.md", side: "worktree" });
    expect(screen.getByRole("tab", { name: "Preview" }).getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector(".file-viewer-preview h1")?.textContent).toBe("Title");
    fireEvent.click(screen.getByRole("tab", { name: "File" }));
    expect(rows()).toHaveLength(3);
  });

  test("a file that is not Markdown offers no preview", async () => {
    await open();
    expect(screen.queryByRole("tab", { name: "Preview" })).toBeNull();
  });

  /// Chromium's scrollIntoView returns a promise; returned from the effect,
  /// React took it for the cleanup and threw on the next tab.
  test("showing another tab scrolls it into view and keeps working", async () => {
    const scroll = HTMLElement.prototype.scrollIntoView;
    const scrolled: string[] = [];
    HTMLElement.prototype.scrollIntoView = function (this: HTMLElement) {
      scrolled.push(this.title);
      return Promise.resolve() as unknown as void;
    };
    try {
      const a: FileTarget = { path: "a.rs", side: "worktree" };
      const b: FileTarget = { path: "b.rs", side: "worktree" };
      const props = {
        files: [a, b],
        preview: null,
        workspace: "/repo",
        onActivate: () => {},
        onPin: () => {},
        onClose: () => {},
        onCloseAll: () => {},
        wrap: true,
      };
      const shown = render(<FileViewer {...props} active={a} />);
      await settle();
      shown.rerender(<FileViewer {...props} active={b} />);
      await settle();
      expect(scrolled).toEqual(["a.rs", "b.rs"]);
      expect(screen.getByRole("tab", { name: "b.rs" }).getAttribute("aria-selected")).toBe("true");
    } finally {
      HTMLElement.prototype.scrollIntoView = scroll;
    }
  });

  test("steps through the changed files in Changes' order, going round, by button or Alt+arrow", async () => {
    const file = (path: string) => ({ path, add: 1, del: 0 });
    changes = { unstaged: [file("a.rs"), file("b.rs")], staged: [file("c.rs")] };
    const shown: FileTarget[] = [];
    const b: FileTarget = { path: "b.rs", side: "unstaged" };
    render(
      <FileViewer
        files={[b]}
        active={b}
        preview={null}
        workspace="/repo"
        onActivate={(t) => shown.push(t)}
        onPin={() => {}}
        onClose={() => {}}
        onCloseAll={() => {}}
        wrap
      />,
    );
    await settle();
    expect(document.querySelector(".file-viewer-step-count")?.textContent).toBe("2 / 3");
    fireEvent.click(screen.getByTitle("Next changed file (Alt+↓)"));
    fireEvent.click(screen.getByTitle("Previous changed file (Alt+↑)"));
    fireEvent.keyDown(document.querySelector(".file-viewer")!, { key: "ArrowDown", code: "ArrowDown", altKey: true });
    expect(shown).toEqual([
      { path: "c.rs", side: "staged" },
      { path: "a.rs", side: "unstaged" },
      { path: "c.rs", side: "staged" },
    ]);
  });

  test("with nothing changed there is nothing to step through", async () => {
    await open();
    expect(document.querySelector(".file-viewer-step")).toBeNull();
  });

  test("long lines wrap as Settings says", async () => {
    const wrapped = () => document.querySelector(".diff-view")?.classList.contains("diff-view-wrap");
    const shown = await open();
    expect(wrapped()).toBe(true);
    shown.unmount();
    await open(undefined, () => {}, false);
    expect(wrapped()).toBe(false);
  });

  /// The IDE layout's one row: the controls beside the tabs, the path left to the tab's tooltip.
  test("compact, the heading is one row and a changed file opens whole", async () => {
    let closed = 0;
    await open(undefined, () => closed++, true, true);
    expect(document.querySelector(".file-viewer-head")).toBeNull();
    const row = document.querySelector(".file-viewer-tabs")!;
    expect(row.querySelector(".tabs")?.textContent).toBe("DiffFile");
    expect(row.textContent).toContain("Unstaged");
    expect(row.textContent).toContain("+1");
    expect(screen.queryByTitle("Close all")).toBeNull();
    expect(screen.getByRole("tab", { name: "main.rs" }).closest("[title]")?.getAttribute("title")).toBe("src/main.rs");
    // Opened to be read: the whole file with its change in place, the diff a click away.
    expect(screen.getByRole("tab", { name: "File" }).getAttribute("aria-selected")).toBe("true");
    expect(rows()).toHaveLength(11);
    fireEvent.click(screen.getByRole("tab", { name: "Diff" }));
    expect(rows()).toHaveLength(9);
    fireEvent.keyDown(row, { key: "Escape" });
    expect(closed).toBe(1);
  });

  test("compact, a changed Markdown file opens as it renders", async () => {
    view = { old: "# Title\n", new: "# Title\n\nMore.\n", unviewable: null };
    await open({ path: "docs/README.md", side: "unstaged" }, () => {}, true, true);
    expect(screen.getByRole("tab", { name: "Preview" }).getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector(".file-viewer-preview h1")?.textContent).toBe("Title");
  });
});
