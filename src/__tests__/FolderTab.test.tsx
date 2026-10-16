import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { FolderTab, folderOptions } from "../components/FolderTab";
import type { ChangeTotals, RecentFolder } from "../lib/chat";
import { fromSnapshot, type IndexState } from "../lib/indexStatus";

// The strip on the composer's top edge: which folder the next message is
// worked on, and what that folder is like — its branch, index and changes.

type Over = {
  path?: string | null;
  recent?: string[];
  branch?: string | null;
  index?: IndexState | null;
  changes?: ChangeTotals | null;
  onOpenFolder?: (path: string) => void;
  onPickFolder?: () => void;
  onOpenChanges?: () => void;
};

const tab = (over: Over = {}) =>
  render(
    <FolderTab
      path={over.path === undefined ? "/work/kibo" : over.path}
      recent={(over.recent ?? []).map((path) => ({ path, worktreeOf: null }))}
      branch={over.branch}
      index={over.index}
      changes={over.changes}
      onOpenFolder={over.onOpenFolder ?? (() => {})}
      onPickFolder={over.onPickFolder ?? (() => {})}
      onOpenChanges={over.onOpenChanges ?? (() => {})}
    />,
  );

const trigger = () => screen.getByRole("button", { expanded: false });

describe("the folder", () => {
  test("asks for one while none is open, and shows nothing else", () => {
    let picks = 0;
    tab({ path: null, branch: "main", changes: { files: 1, add: 1, del: 0 }, onPickFolder: () => picks++ });
    expect(trigger().textContent).toContain("Select folder");
    expect(screen.queryByText("main")).toBeNull();
    expect(screen.queryByTitle(/changed since the last commit/)).toBeNull();

    fireEvent.click(trigger());
    fireEvent.click(screen.getByRole("option", { name: /Open folder/ }));
    expect(picks).toBe(1);
  });

  /// Picking the folder already open is not a switch: it would reload its chats for nothing.
  test("names the open one, the whole path on hover, and switches only to another", () => {
    const opened: string[] = [];
    tab({ recent: ["/work/kibo", "/work/atlas"], onOpenFolder: (p) => opened.push(p) });
    expect(trigger().textContent).toContain("kibo");
    expect(trigger().getAttribute("title")).toBe("/work/kibo");

    fireEvent.click(trigger());
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual([
      "kibo/work/kibo",
      "atlas/work/atlas",
      "Open folder…",
    ]);
    fireEvent.click(screen.getByRole("option", { name: /^kibo/ }));
    fireEvent.click(trigger());
    fireEvent.click(screen.getByRole("option", { name: /^atlas/ }));
    expect(opened).toEqual(["/work/atlas"]);
  });
});

describe("the branch", () => {
  test("is shown where there is one", () => {
    tab({ branch: "feature/parser" });
    expect(screen.getByTitle("Checked-out branch").textContent).toBe("feature/parser");
  });

  test("outside a repository there is none", () => {
    tab({ branch: null });
    expect(screen.queryByTitle("Checked-out branch")).toBeNull();
  });
});

describe("the index", () => {
  test("its state is shown, with the detail on hover", () => {
    tab({ index: fromSnapshot({ root: "/work/kibo", syncing: false, embedded: 3, skipped: 2, embeddingError: null }) });
    const badge = screen.getByRole("status");
    expect(badge.textContent).toBe("Indexed");
    expect(badge.getAttribute("title")).toContain("2 files not indexed");
  });

  test("nothing is shown before anything is known", () => {
    tab();
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("the changes", () => {
  test("shows lines added and removed, and opens Changes", () => {
    let opened = 0;
    tab({ changes: { files: 2, add: 12, del: 3 }, onOpenChanges: () => opened++ });
    const button = screen.getByTitle("2 files changed since the last commit");
    expect(button.textContent).toBe("+12−3");
    fireEvent.click(button);
    expect(opened).toBe(1);
  });

  test("a clean folder, or no repository, shows nothing", () => {
    tab({ changes: { files: 0, add: 0, del: 0 } });
    expect(screen.queryByTitle(/changed since the last commit/)).toBeNull();
  });
});

describe("the folder menu", () => {
  const folder = (path: string, worktreeOf: string | null = null): RecentFolder => ({ path, worktreeOf });

  test("puts a worktree under the folder it was made from, however recently each was opened", () => {
    const rows = folderOptions([
      folder("/home/.kibo/worktrees/kibo/main-0925-2214", "/work/kibo"),
      folder("/work/atlas"),
      folder("/work/kibo/"),
      folder("/home/.kibo/worktrees/kibo/fix-0924-1010", "/work/kibo"),
    ]);
    expect(rows.map((row) => [row.label, row.hint, Boolean(row.nested)])).toEqual([
      ["atlas", "/work/atlas", false],
      ["kibo", "/work/kibo/", false],
      ["main-0925-2214", "worktree of kibo", true],
      ["fix-0924-1010", "worktree of kibo", true],
    ]);
  });

  test("a worktree whose folder is not listed stays in place, named as one but not drawn under another", () => {
    const rows = folderOptions([folder("/work/atlas"), folder("/wt/kibo/main-0925-2214", "/work/kibo")]);
    expect(rows.map((row) => [row.label, row.hint, Boolean(row.nested)])).toEqual([
      ["atlas", "/work/atlas", false],
      ["main-0925-2214", "worktree of kibo", false],
    ]);
  });

  test("drops a folder from the list, without opening it", () => {
    const forgotten: string[] = [];
    const opened: string[] = [];
    render(
      <FolderTab
        path="/work/kibo"
        recent={[folder("/work/kibo"), folder("/work/atlas")]}
        onOpenFolder={(path) => opened.push(path)}
        onPickFolder={() => {}}
        onForgetFolder={(path) => forgotten.push(path)}
        onOpenChanges={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { expanded: false }));
    const buttons = screen.getAllByTitle("Remove from the list — the folder itself is left as it is");
    expect(buttons).toHaveLength(2);

    fireEvent.click(buttons[1]);
    expect(forgotten).toEqual(["/work/atlas"]);
    expect(opened).toEqual([]);
    expect(screen.queryByRole("option")).toBeNull();
  });

  test("draws the nested rows as such", () => {
    render(
      <FolderTab
        path="/work/kibo"
        recent={[folder("/work/kibo"), folder("/wt/kibo/main-0925-2214", "/work/kibo")]}
        onOpenFolder={() => {}}
        onPickFolder={() => {}}
        onOpenChanges={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { expanded: false }));
    const [main, worktree] = screen.getAllByRole("option");
    expect(main.className).not.toContain("nested");
    expect(worktree.className).toContain("nested");
    expect(worktree.textContent).toBe("main-0925-2214worktree of kibo");
  });
});
