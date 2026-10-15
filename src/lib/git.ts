import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";
import { onIndexEvent } from "./chat";

// The open folder in git: changes, staging, commits, history, branches, worktrees, files.

/** Mirrors `domain::git_changes::ChangedFile`; `path` is relative to the repository. */
export type ChangedFile = { path: string; add: number; del: number };
export type WorkingChanges = { staged: ChangedFile[]; unstaged: ChangedFile[] };

/**
 * The open folder's repository changed outside the tree — a `git add` or a
 * commit made elsewhere. Pinned on the Rust side in `commands/git.rs`.
 */
export const GIT_EVENT = "workspace-git:changed";

export async function onGitChanged(handler: (root: string) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ root: string }>(GIT_EVENT, ({ payload }) => handler(payload.root));
}

/**
 * Something in the folder `root` changed: its tree (the index's watcher
 * starting a sync) or its `.git`, which that watcher leaves out. The two
 * signals a view of the folder reads again on, as one. No folder (`null`)
 * hears nothing.
 */
export async function onFolderChanged(root: string | null, handler: () => void): Promise<UnlistenFn> {
  const stops = await Promise.all([
    onIndexEvent((event) => event.root === root && event.kind === "syncStarted" && handler()),
    onGitChanged((changed) => changed === root && handler()),
  ]);
  return () => stops.forEach((stop) => stop());
}

/** Mirrors `domain::git_changes::ChangeTotals`: the open folder against the last commit. */
export type ChangeTotals = { files: number; add: number; del: number };

/** `null` outside a repository, and outside the desktop app. */
export async function gitTotals(): Promise<ChangeTotals | null> {
  if (!inTauri()) return null;
  return invoke<ChangeTotals | null>("git_totals");
}

export async function gitChanges(): Promise<WorkingChanges> {
  requireBackend();
  return invoke<WorkingChanges>("git_changes");
}

export async function gitStage(paths: string[]): Promise<void> {
  requireBackend();
  return invoke("git_stage", { paths });
}

export async function gitUnstage(paths: string[]): Promise<void> {
  requireBackend();
  return invoke("git_unstage", { paths });
}

/** Commits what is staged; resolves to the new commit's short id. */
export async function gitCommit(message: string): Promise<string> {
  requireBackend();
  return invoke<string>("git_commit", { message });
}

/** A message for what is staged, written by the model; `draft` is what the box already holds. */
export async function gitCommitMessage(draft: string): Promise<string> {
  requireBackend();
  return invoke<string>("git_commit_message", { draft });
}

/** Mirrors `domain::git_branches::BranchRef`: `origin/x` is remote, listed only with no local `x`. */
export type BranchRef = { name: string; remote: boolean };
/** Mirrors `domain::git_branches::CheckoutOutcome`: a conflict left the folder exactly as it was. */
export type CheckoutOutcome = { kind: "switched" } | { kind: "conflicts"; paths: string[] };

/** Local branches first, then remote ones. */
export async function gitBranches(): Promise<BranchRef[]> {
  requireBackend();
  return invoke<BranchRef[]>("git_branches");
}

/** Switches the open folder, or names the files that stand in the way — never overwrites them. */
export async function gitCheckout(branch: string): Promise<CheckoutOutcome> {
  requireBackend();
  return invoke<CheckoutOutcome>("git_checkout", { branch });
}

/** A new worktree on a new branch from `base`; resolves to its folder, not yet opened. */
export async function gitWorktreeAdd(base: string): Promise<string> {
  requireBackend();
  return invoke<string>("git_worktree_add", { base });
}

/** Mirrors `commands::git::WorktreeCheck`: a worktree as removing it would find it. */
export type WorktreeCheck = {
  /** The main folder it was made from. */
  main: string;
  branch: string | null;
  /** Uncommitted changes and untracked files: removal is refused while there are any. */
  dirty: string[];
  /** Commits only this worktree's branch has. */
  ownCommits: number;
  keepsBranch: boolean;
  chats: number;
};
/** Mirrors `commands::git::WorktreeRemoved`. */
export type WorktreeRemoved = { branchKept: string | null; chatsRemoved: number };

/** `path` is a worktree from the folder list, not the open folder. */
export async function gitWorktreeCheck(path: string): Promise<WorktreeCheck> {
  requireBackend();
  return invoke<WorktreeCheck>("git_worktree_check", { path });
}

/** Removes the worktree at `path` — from the folder list, not the open folder — and its chats. */
export async function gitWorktreeRemove(path: string): Promise<WorktreeRemoved> {
  requireBackend();
  return invoke<WorktreeRemoved>("git_worktree_remove", { path });
}

/** Mirrors `domain::git_changes::CommitSummary`; `time` is in seconds since the epoch. */
export type CommitSummary = { id: string; summary: string; author: string; time: number; head: boolean; refs: string[] };
/** Mirrors `domain::git_changes::GitHistory`. */
export type GitHistory = {
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  commits: CommitSummary[];
  more: boolean;
};

/** Where HEAD is and the newest `limit` commits from it, newest first. */
export async function gitHistory(limit: number): Promise<GitHistory> {
  requireBackend();
  return invoke<GitHistory>("git_history", { limit });
}

/**
 * Mirrors `domain::git_changes::FileSide`: which two versions the viewer
 * compares. `unstaged` and `staged` take the repository's paths, as Changes
 * lists them; `worktree` takes the open folder's, as Files does.
 */
export type FileSide = "unstaged" | "staged" | "worktree";
/** A file for the viewer to show, and against what. */
export type FileTarget = { path: string; side: FileSide };
/** Mirrors `domain::git_changes::FileView`: `null` on a side the file is not on. */
export type FileView = { old: string | null; new: string | null; unviewable: "binary" | "tooLarge" | null };

export async function fileView(target: FileTarget): Promise<FileView> {
  requireBackend();
  return invoke<FileView>("file_view", target);
}

/** Mirrors `domain::file_write::FileSave`: changed on disk since it was opened, nothing was written. */
export type FileSave = { kind: "saved" } | { kind: "changedOnDisk" };

/**
 * Saves a file of the open folder, by its path in it, over `expected` — the
 * text it was opened with — or, `null`, over whatever is there now. `root` is
 * the folder the edit was made in: another one open by now, nothing is written.
 */
export async function fileWrite(root: string, path: string, expected: string | null, content: string): Promise<FileSave> {
  requireBackend();
  return invoke<FileSave>("file_write", { root, path, expected, content });
}
