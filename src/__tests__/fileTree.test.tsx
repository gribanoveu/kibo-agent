import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { FolderListing, TreeEntry } from "../lib/chat";

// The Files tab's folder tree: one folder read at a time as it is unfolded,
// git's marks on each entry, and every unfolded folder read again when the
// backend says the open folder changed.

const entry = (path: string, over: Partial<TreeEntry> = {}): TreeEntry => ({
  name: path.split("/").pop()!,
  path,
  isDir: false,
  status: null,
  changed: false,
  ...over,
});

let folders: Record<string, FolderListing>;
let asked: string[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command === "workspace_list") {
      const dir = args!.dir as string;
      asked.push(dir);
      return folders[dir] ? Promise.resolve(structuredClone(folders[dir])) : Promise.reject(`no folder ${dir}`);
    }
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

const { FilesPanel } = await import("../components/FilesPanel");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  folders = {
    "": {
      entries: [
        entry("src", { isDir: true, changed: true }),
        entry("README.md"),
        entry("new.txt", { status: "untracked" }),
      ],
      more: 0,
    },
    src: { entries: [entry("src/lib", { isDir: true }), entry("src/main.rs", { status: "modified" })], more: 3 },
    "src/lib": { entries: [entry("src/lib/deep.rs")], more: 0 },
  };
  asked = [];
  opened = [];
});

let opened: unknown[] = [];
const tab = (active = true) => (
  <FilesPanel active={active} workspace="/repo" blocks={[]} onOpenFile={(target, pin) => opened.push({ ...target, pin })} />
);

/** The tab, on its "All files" view. */
async function allFiles(active = true) {
  const view = render(tab(active));
  fireEvent.click(screen.getByRole("tab", { name: "All files" }));
  await settle();
  return view;
}

describe("the folder tree", () => {
  test("is read only once its own view is picked", async () => {
    render(tab());
    await settle();
    expect(asked).toEqual([]);
    expect(screen.getByRole("tab", { name: "In this chat" }).getAttribute("aria-selected")).toBe("true");
    fireEvent.click(screen.getByRole("tab", { name: "All files" }));
    await settle();
    expect(asked).toEqual([""]);
    expect(screen.getByRole("tab", { name: "All files" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("tab", { name: "All files" }).title).toBe("/repo");
  });

  test("shows the open folder's top level with git's marks, and nothing unfolded", async () => {
    await allFiles();
    expect(screen.getByText("README.md")).toBeTruthy();
    expect(screen.getByTitle("Untracked").textContent).toBe("U");
    expect(screen.getByTitle("Changes inside")).toBeTruthy();
    expect(screen.queryByText("main.rs")).toBeNull();
    expect(asked).toEqual([""]);
  });

  test("a folder unfolds on a click, reads its own entries, and folds again", async () => {
    await allFiles();
    fireEvent.click(screen.getByText("src"));
    await settle();
    expect(asked).toEqual(["", "src"]);
    expect(screen.getByText("main.rs")).toBeTruthy();
    expect(screen.getByTitle("Modified").textContent).toBe("M");
    expect(screen.getByText("…and 3 more")).toBeTruthy();
    expect(screen.getByText("src").closest("button")!.getAttribute("aria-expanded")).toBe("true");

    fireEvent.click(screen.getByText("lib"));
    await settle();
    expect(screen.getByText("deep.rs")).toBeTruthy();

    fireEvent.click(screen.getByText("src"));
    await settle();
    expect(screen.queryByText("main.rs")).toBeNull();
    expect(screen.queryByText("deep.rs")).toBeNull();
  });

  test("a change to the folder reads the top and every unfolded folder again, for this folder only", async () => {
    await allFiles();
    fireEvent.click(screen.getByText("src"));
    await settle();
    asked = [];

    folders.src.entries[1] = entry("src/main.rs");
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(asked.sort()).toEqual(["", "src"]);
    expect(screen.queryByTitle("Modified")).toBeNull();

    asked = [];
    act(() => emit("workspace-index:event", { root: "/repo", kind: "syncStarted" }));
    await settle();
    expect(asked.sort()).toEqual(["", "src"]);

    asked = [];
    act(() => {
      emit("workspace-git:changed", { root: "/elsewhere" });
      emit("workspace-index:event", { root: "/repo", kind: "keywordsReady" });
    });
    await settle();
    expect(asked).toEqual([]);
  });

  test("hidden, it asks nothing and hears nothing", async () => {
    const view = await allFiles(false);
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(asked).toEqual([]);
    view.rerender(tab(true));
    await settle();
    expect(asked).toEqual([""]);
  });

  test("a folder holding only a folder unfolds on down to where there is a file or a choice", async () => {
    folders[""] = { entries: [entry("app", { isDir: true }), entry("README.md")], more: 0 };
    folders.app = { entries: [entry("app/main", { isDir: true })], more: 0 };
    folders["app/main"] = { entries: [entry("app/main/java", { isDir: true })], more: 0 };
    // Two folders: the choice is the reader's.
    folders["app/main/java"] = { entries: [entry("app/main/java/a", { isDir: true }), entry("app/main/java/b", { isDir: true })], more: 0 };
    folders["app/main/java/a"] = { entries: [entry("app/main/java/a/only", { isDir: true })], more: 0 };
    await allFiles();

    fireEvent.click(screen.getByText("app"));
    await settle();
    expect(asked).toEqual(["", "app", "app/main", "app/main/java"]);
    // The run is one row, and what is under its last folder is one level in.
    const run = screen.getByTitle("app/main/java");
    expect(run.textContent).toBe("app / main / java");
    expect(run.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getAllByRole("button", { expanded: true })).toHaveLength(1);
    const a = screen.getByTitle("app/main/java/a");
    expect(a.getAttribute("aria-expanded")).toBe("false");
    expect(a.style.getPropertyValue("--depth")).toBe("1");
    expect(screen.getByTitle("app/main/java/b")).toBeTruthy();

    // A click on the run folds all of it; another brings it back as it was.
    fireEvent.click(run);
    await settle();
    expect(screen.queryByTitle("app/main/java/a")).toBeNull();
    expect(screen.getByTitle("app").textContent).toBe("app");
    fireEvent.click(screen.getByTitle("app"));
    await settle();
    expect(screen.getByTitle("app/main/java").textContent).toBe("app / main / java");

    // A change reads what is open again, and opens nothing more.
    asked = [];
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(asked.sort()).toEqual(["", "app", "app/main", "app/main/java"]);
    expect(screen.queryByTitle("app/main/java/a/only")).toBeNull();
  });

  test("a single folder with a file beside it is where the unfolding stops", async () => {
    folders.src = { entries: [entry("src/lib", { isDir: true }), entry("src/main.rs")], more: 0 };
    await allFiles();
    fireEvent.click(screen.getByText("src"));
    await settle();
    expect(asked).toEqual(["", "src"]);
    expect(screen.getByTitle("src").textContent).toBe("src");
    expect(screen.getByTitle("src/lib").getAttribute("aria-expanded")).toBe("false");
  });

  test("a folder unfolded beside a file stays a row of its own", async () => {
    folders.src = { entries: [entry("src/lib", { isDir: true }), entry("src/main.rs")], more: 0 };
    await allFiles();
    fireEvent.click(screen.getByTitle("src"));
    await settle();
    fireEvent.click(screen.getByTitle("src/lib"));
    await settle();
    expect(screen.getByTitle("src").textContent).toBe("src");
    expect(screen.getByTitle("src/lib").getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("main.rs")).toBeTruthy();
  });

  test("a folder left holding one folded folder does not take it into its row", async () => {
    folders.src = { entries: [entry("src/lib", { isDir: true }), entry("src/main.rs")], more: 0 };
    await allFiles();
    fireEvent.click(screen.getByTitle("src"));
    await settle();

    // main.rs is deleted elsewhere: src now holds only lib, which nobody unfolded.
    folders.src = { entries: [entry("src/lib", { isDir: true })], more: 0 };
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(screen.getByTitle("src").textContent).toBe("src");
    expect(screen.getByTitle("src/lib").getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Loading…")).toBeNull();
  });

  test("a folder that cannot be read says why", async () => {
    delete folders[""];
    await allFiles();
    expect(screen.getByText("no folder")).toBeTruthy();
  });

  test("a file opens in the viewer by its path in the folder", async () => {
    await allFiles();
    fireEvent.click(screen.getByText("src"));
    await settle();
    fireEvent.click(screen.getByText("main.rs"));
    fireEvent.doubleClick(screen.getByText("main.rs"));
    expect(opened).toEqual([
      { path: "src/main.rs", side: "worktree", pin: false },
      { path: "src/main.rs", side: "worktree", pin: true },
    ]);
  });

  /// Only the file the viewer shows is marked: an unfolded folder on the way
  /// to it is not drawn as open too.
  test("the shown file is marked, the folders unfolded above it are not", async () => {
    render(
      <FilesPanel
        active
        workspace="/repo"
        blocks={[]}
        openFile={{ path: "src/main.rs", side: "worktree" }}
        onOpenFile={() => {}}
        docs
      />,
    );
    await settle();
    fireEvent.click(screen.getByText("src"));
    await settle();
    const src = screen.getByText("src").closest("button")!;
    expect(src.className).not.toContain("open");
    expect(src.className).toContain("expanded");
    expect(screen.getByText("main.rs").closest("button")!.className).toContain("open");
  });
});
