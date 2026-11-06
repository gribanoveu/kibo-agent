import { Folder, FolderPlus, GitBranch, GitFork, Trash2, Undo2, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import type { RecentFolder } from "../lib/chat";
import type { BranchRef } from "../lib/git";
import "./FolderTab.css";

/** Not a path: no folder is called this. */
const PICK = "\u0000pick";

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/**
 * The folder menu's rows: each folder the last one first, and a worktree
 * straight under the folder it was made from, marked as belonging to it. One
 * whose folder is not in the list stays where it falls, said to be one but
 * not drawn under a folder it does not belong to.
 */
export function folderOptions(recent: RecentFolder[]) {
  const trimmed = (path: string) => path.replace(/[\\/]+$/, "");
  const listed = new Set(recent.map((one) => trimmed(one.path)));
  const row = (one: RecentFolder, nested = false) =>
    one.worktreeOf
      ? { value: one.path, label: folderName(one.path), hint: `worktree of ${folderName(one.worktreeOf)}`, nested, worktree: true }
      : { value: one.path, label: folderName(one.path), hint: one.path, worktree: false };
  return recent.flatMap((one) => {
    if (one.worktreeOf) return listed.has(trimmed(one.worktreeOf)) ? [] : [row(one)];
    const own = recent.filter((other) => other.worktreeOf && trimmed(other.worktreeOf) === trimmed(one.path));
    return [row(one), ...own.map((other) => row(other, true))];
  });
}

type Props = {
  /** The open folder; `null` when nothing is open. */
  path: string | null;
  /** Folders opened lately, the last one first. */
  recent: RecentFolder[];
  onOpenFolder: (path: string) => void;
  onPickFolder: () => void;
  /** Offered on each worktree in the folder menu, except the open one. */
  onRemoveWorktree?: (path: string) => void;
  /** Offered on every folder in the menu: the entry goes, nothing else. */
  onForgetFolder?: (path: string) => void;
  /** The main working tree, when the folder is a git worktree of it. */
  worktreeOf?: string | null;
  /** The git branch checked out in the folder; `null` outside a repository. */
  branch?: string | null;
  /** Switch the folder's branch, or start a worktree from one. Without it
      the branch is only shown. */
  branchPicker?: {
    branches: BranchRef[];
    onOpen: () => void;
    onPick: (name: string) => void;
    worktree: boolean;
    /** Where a worktree starts, shown in place of the branch while Worktree is on. */
    base?: string | null;
    /** Why the branch cannot be switched now — a turn is working in it. */
    locked?: string;
  };
  /** The branch the open chat works on. Another chat of the folder may have
      moved the folder off it since: then it is said, with the way back. */
  chatBranch?: string | null;
};

/** Where the next message will be worked on: the folder and its branch. Sits
    right under the composer — this is read before sending, not looked up in the
    sidebar. */
export function FolderTab({
  path,
  recent,
  onOpenFolder,
  onPickFolder,
  onRemoveWorktree,
  onForgetFolder,
  branch = null,
  worktreeOf = null,
  branchPicker,
  chatBranch = null,
}: Props) {
  const shownBranch = (branchPicker?.worktree && branchPicker.base) || branch || "";
  const away = chatBranch && branch && chatBranch !== branch ? chatBranch : null;
  return (
    <div className={`folder-tab${path ? "" : " no-folder"}`}>
      <Dropdown
        title={
          path && worktreeOf
            ? `A worktree of ${worktreeOf}\nThis folder: ${path}`
            : path ?? "Choose the folder the agent works in"
        }
        heading="Folder"
        label={
          path && worktreeOf ? (
            // Named after the repository it belongs to: the worktree's own
            // folder name says no more than the branch beside it.
            <span className="folder-tab-name in-worktree">
              <GitFork size={13} />
              <span className="folder-tab-text">{folderName(worktreeOf)}</span>
              <span className="folder-tab-tag">worktree</span>
            </span>
          ) : (
            <span className="folder-tab-name">
              {path ? <Folder size={13} /> : <FolderPlus size={13} />}
              <span className="folder-tab-text">{path ? folderName(path) : "Select folder"}</span>
            </span>
          )
        }
        value={path ?? ""}
        options={[
          ...folderOptions(recent).map(({ worktree, ...row }) => {
            if (worktree && onRemoveWorktree) {
              return {
                ...row,
                action: {
                  icon: <Trash2 size={13} />,
                  // The open one runs processes and terminals, watched
                  // and indexed; they stop only when another folder opens.
                  title:
                    row.value === path
                      ? "This worktree is open — switch to another folder to remove it"
                      : "Remove this worktree…",
                  unavailable: row.value === path,
                  onRun: () => onRemoveWorktree(row.value),
                },
              };
            }
            if (!onForgetFolder) return row;
            // The folder itself is left alone, so the open one is offered
            // too: the entry goes, and the next launch opens another.
            return {
              ...row,
              action: {
                icon: <X size={13} />,
                title: "Remove from the list — the folder itself is left as it is",
                onRun: () => onForgetFolder(row.value),
              },
            };
          }),
          { value: PICK, label: "Open folder…" },
        ]}
        onPick={(value) => (value === PICK ? onPickFolder() : value !== path && onOpenFolder(value))}
      />
      {path && branch && branchPicker && (
        <>
          <Dropdown
            locked={!!branchPicker.locked && !branchPicker.worktree}
            title={
              (branchPicker.locked && !branchPicker.worktree
                ? branchPicker.locked
                : undefined) ??
              (branchPicker.worktree ? "The branch a new worktree starts from" : "Switch the folder's branch")
            }
            heading={branchPicker.worktree ? "Start a worktree from" : "Branch"}
            label={
              <span className="folder-tab-name">
                <GitBranch size={12} />
                <span className="folder-tab-text">{shownBranch}</span>
              </span>
            }
            value={shownBranch}
            options={branchPicker.branches.map((one) => ({ value: one.name, hint: one.remote ? "remote" : undefined }))}
            emptyLabel="Reading branches…"
            onOpen={branchPicker.onOpen}
            onPick={branchPicker.onPick}
          />
          {away && (
            <button
              type="button"
              className="folder-tab-away"
              title={branchPicker.locked ?? `This chat works on ${away}; the folder is on ${branch} now. Switch back to ${away}`}
              aria-disabled={!!branchPicker.locked || undefined}
              onClick={() => {
                if (!branchPicker.locked) branchPicker.onPick(away);
              }}
            >
              <Undo2 size={12} />
              <span className="folder-tab-text">{away}</span>
            </button>
          )}
        </>
      )}
      {path && branch && !branchPicker && (
        <span className="folder-tab-branch" title="Checked-out branch">
          <GitBranch size={12} />
          <span>{branch}</span>
        </span>
      )}
    </div>
  );
}
