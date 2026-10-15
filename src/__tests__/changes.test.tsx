import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { GitHistory, WorkingChanges } from "../lib/git";

// The Changes tab: the repository's unstaged and staged files with their line
// counts, moved between the two by the row's button, and committed.

let repo: WorkingChanges;
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let commitFails: string | null = null;
let history: GitHistory;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    const paths = (args?.paths ?? []) as string[];
    const move = (from: "staged" | "unstaged", to: "staged" | "unstaged") => {
      const moved = repo[from].filter((f) => paths.includes(f.path));
      repo = { ...repo, [from]: repo[from].filter((f) => !paths.includes(f.path)), [to]: [...repo[to], ...moved] } as WorkingChanges;
    };
    if (command === "git_changes") return Promise.resolve(structuredClone(repo));
    if (command === "git_history") return Promise.resolve(history);
    if (command === "git_stage") return Promise.resolve(move("unstaged", "staged"));
    if (command === "git_unstage") return Promise.resolve(move("staged", "unstaged"));
    if (command === "git_commit") {
      if (commitFails) return Promise.reject(commitFails);
      repo = { ...repo, staged: [] };
      return Promise.resolve("abc1234");
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

/** The backend's events, by channel: what each listener would hear. */
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

const { ChangesPanel } = await import("../components/ChangesPanel");
const { ago } = await import("../components/HistoryView");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  repo = {
    staged: [],
    unstaged: [
      { path: "src/main/tax/TaxProfileMapper.java", add: 4, del: 1 },
      { path: "README.md", add: 3, del: 0 },
    ],
  };
  calls = [];
  opened = [];
  commitFails = null;
  history = { branch: "main", upstream: null, ahead: 0, behind: 0, commits: [], more: false };
});

let opened: unknown[] = [];

function panel(message = "", notes: string[] = [], messages: string[] = []) {
  return render(
    <ChangesPanel
      active
      workspace="/repo"
      onNotify={(note) => notes.push(note)}
      onOpenFile={(target, pin) => opened.push(pin ? { ...target, pin } : target)}
      message={message}
      onMessage={(next) => messages.push(next)}
    />,
  );
}

describe("ChangesPanel", () => {
  test("lists the changed files by name, and folder, with their counts", async () => {
    panel();
    await settle();
    const name = screen.getByText("TaxProfileMapper.java");
    expect(name.parentElement?.title).toBe("src/main/tax/TaxProfileMapper.java — show diff");
    expect(name.parentElement?.textContent).toBe("TaxProfileMapper.javasrc/main/tax");
    // A file at the root has no folder line.
    expect(screen.getByText("README.md").parentElement?.textContent).toBe("README.md");
    expect(screen.getByText("+4")).toBeTruthy();
    expect(screen.getByText("-1")).toBeTruthy();
    expect(screen.getByText("Stage files to commit")).toBeTruthy();
  });

  test("a row's button stages it by path, and the lists are read back", async () => {
    panel();
    await settle();
    fireEvent.click(screen.getAllByTitle("Stage")[0]);
    await settle();
    expect(calls.find((c) => c.command === "git_stage")?.args).toEqual({ paths: ["src/main/tax/TaxProfileMapper.java"] });
    expect(screen.getAllByTitle("Unstage")).toHaveLength(1);

    fireEvent.click(screen.getByTitle("Unstage"));
    await settle();
    expect(calls.find((c) => c.command === "git_unstage")?.args).toEqual({ paths: ["src/main/tax/TaxProfileMapper.java"] });
    expect(screen.queryByTitle("Unstage")).toBeNull();
  });

  test("a file's name opens it in the viewer, against the side it is listed on", async () => {
    repo = { ...repo, staged: [{ path: "src/Staged.java", add: 1, del: 0 }] };
    panel();
    await settle();
    fireEvent.click(screen.getByText("README.md"));
    fireEvent.click(screen.getByText("Staged.java"));
    fireEvent.doubleClick(screen.getByText("Staged.java"));
    expect(opened).toEqual([
      { path: "README.md", side: "unstaged" },
      { path: "src/Staged.java", side: "staged" },
      { path: "src/Staged.java", side: "staged", pin: true },
    ]);
  });

  test("the file the viewer shows is marked, on its own side only", async () => {
    repo = { ...repo, staged: [{ path: "README.md", add: 1, del: 0 }] };
    render(
      <ChangesPanel
        active
        workspace="/repo"
        onNotify={() => {}}
        openFile={{ path: "README.md", side: "staged" }}
        onOpenFile={() => {}}
        message=""
        onMessage={() => {}}
      />,
    );
    await settle();
    const marked = [...document.querySelectorAll(".stage-file.open")];
    expect(marked).toHaveLength(1);
    expect(marked[0].querySelector(".stage-btn")?.getAttribute("title")).toBe("Unstage");
  });

  test("Stage all stages every unstaged file at once", async () => {
    panel();
    await settle();
    fireEvent.click(screen.getByText("Stage all"));
    await settle();
    expect(calls.find((c) => c.command === "git_stage")?.args).toEqual({
      paths: ["src/main/tax/TaxProfileMapper.java", "README.md"],
    });
    expect(screen.getByText("No unstaged changes")).toBeTruthy();
  });

  test("a commit sends the message, clears it and says the id", async () => {
    repo = { staged: repo.unstaged, unstaged: [] };
    const notes: string[] = [];
    const messages: string[] = [];
    panel("Fix the tax rate", notes, messages);
    await settle();
    fireEvent.click(screen.getByText("Commit"));
    await settle();
    expect(calls.find((c) => c.command === "git_commit")?.args).toEqual({ message: "Fix the tax rate" });
    expect(messages).toEqual([""]);
    expect(notes).toEqual(["Committed abc1234"]);
  });

  test("a failed commit keeps the message and says why", async () => {
    repo = { staged: repo.unstaged, unstaged: [] };
    commitFails = "git user.name and user.email are not set";
    const notes: string[] = [];
    const messages: string[] = [];
    panel("Fix", notes, messages);
    await settle();
    fireEvent.click(screen.getByText("Commit"));
    await settle();
    expect(messages).toEqual([]);
    expect(notes).toEqual(["git user.name and user.email are not set"]);
  });

  test("Commit waits for something staged and a message", async () => {
    panel("Fix");
    await settle();
    expect((screen.getByText("Commit") as HTMLButtonElement).disabled).toBe(true);
  });

  test("reads again when the folder or its .git changes, and only for the open folder", async () => {
    const view = panel();
    await settle();
    const reads = () => calls.filter((c) => c.command === "git_changes").length;
    const before = reads();

    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(reads()).toBe(before + 1);

    act(() => emit("workspace-index:event", { root: "/repo", kind: "syncStarted" }));
    await settle();
    expect(reads()).toBe(before + 2);

    // Another folder's events, and the rest of a sync, are not a change here.
    act(() => {
      emit("workspace-git:changed", { root: "/elsewhere" });
      emit("workspace-index:event", { root: "/elsewhere", kind: "syncStarted" });
      emit("workspace-index:event", { root: "/repo", kind: "keywordsReady" });
    });
    await settle();
    expect(reads()).toBe(before + 2);

    // Hidden, the tab stops listening.
    view.rerender(
      <ChangesPanel active={false} workspace="/repo" onNotify={() => {}} onOpenFile={() => {}} message="" onMessage={() => {}} />,
    );
    await settle();
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(reads()).toBe(before + 2);
  });
});

describe("History tab", () => {
  const now = Date.now() / 1000;
  const commit = (id: string, summary: string, age: number, more: Partial<GitHistory["commits"][0]> = {}) => ({
    id,
    summary,
    author: "Eugene",
    time: now - age,
    head: false,
    refs: [],
    ...more,
  });

  async function openHistory() {
    const view = panel();
    await settle();
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    await settle();
    return view;
  }

  test("is read only once its tab is open", async () => {
    panel();
    await settle();
    expect(calls.some((c) => c.command === "git_history")).toBe(false);
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    await settle();
    expect(calls.find((c) => c.command === "git_history")?.args).toEqual({ limit: 50 });
  });

  test("shows the branch against its upstream and each commit with its marks", async () => {
    history = {
      branch: "fix/npe",
      upstream: "origin/fix/npe",
      ahead: 1,
      behind: 2,
      more: false,
      commits: [
        commit("a3f9c2e", "fix: guard zero income", 300, { head: true }),
        commit("7be2c14", "feat: tax mapper", 2 * 86400, { refs: ["origin/main", "v1"] }),
      ],
    };
    await openHistory();
    expect(document.querySelector(".history-branch")?.textContent).toBe("fix/npe↑1↓2origin/fix/npe");
    const rows = [...document.querySelectorAll(".history-commit")];
    expect(rows.map((row) => row.querySelector(".history-meta")?.textContent)).toEqual([
      "HEADa3f9c2e·Eugene·5m ago",
      "origin/mainv17be2c14·Eugene·2d ago",
    ]);
    expect(rows[0].classList.contains("head")).toBe(true);
    expect(rows[1].classList.contains("head")).toBe(false);
    expect(screen.queryByText("Load more")).toBeNull();
  });

  test("Load more asks for the next fifty", async () => {
    history = { ...history, commits: [commit("a3f9c2e", "one", 60)], more: true };
    await openHistory();
    fireEvent.click(screen.getByText("Load more"));
    await settle();
    expect(calls.filter((c) => c.command === "git_history").map((c) => c.args)).toEqual([{ limit: 50 }, { limit: 100 }]);
  });

  test("reads again when .git changes, and says when there is nothing yet", async () => {
    await openHistory();
    expect(screen.getByText("No commits yet")).toBeTruthy();
    const reads = () => calls.filter((c) => c.command === "git_history").length;
    const before = reads();
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    act(() => emit("workspace-git:changed", { root: "/elsewhere" }));
    await settle();
    expect(reads()).toBe(before + 1);
  });
});

describe("ago", () => {
  test("says it as a person would", () => {
    const now = 1_000_000;
    expect(ago(now - 10, now)).toBe("now");
    expect(ago(now - 55 * 60, now)).toBe("55m ago");
    expect(ago(now - 3 * 3600, now)).toBe("3h ago");
    expect(ago(now - 30 * 3600, now)).toBe("yesterday");
    expect(ago(now - 3 * 86400, now)).toBe("3d ago");
    expect(ago(now - 10 * 86400, now)).toBe(new Date((now - 10 * 86400) * 1000).toLocaleDateString());
    // A clock behind the commit's is not the future.
    expect(ago(now + 100, now)).toBe("now");
  });
});
