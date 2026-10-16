import { Check, Folder, FolderPlus, GitBranch, GitFork, Trash2, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { IndexBadge } from "./IndexBadge";
import type { BranchRef, ChangeTotals, RecentFolder } from "../lib/chat";
import type { IndexState } from "../lib/indexStatus";
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
  /** The folder's index; `null` until anything is known about it. */
  index?: IndexState | null;
  /** Lines added and removed since the last commit; `null` outside a repository. */
  changes?: ChangeTotals | null;
  onOpenChanges: () => void;
  /** Offered while the chat has not started: switch the branch, or start a
      worktree from one. Without it the branch is only shown. */
  branchPicker?: {
    branches: BranchRef[];
    onOpen: () => void;
    onPick: (name: string) => void;
    worktree: boolean;
    /** Where a worktree starts, shown in place of the branch while Worktree is on. */
    base?: string | null;
    onWorktree: (on: boolean) => void;
  };
};

/** Where the next message will be worked on: the folder, its branch, its
    index and what it holds against its last commit. Sits on the composer's
    top edge — this is read before sending, not looked up in the sidebar. */
export function FolderTab({
  path,
  recent,
  onOpenFolder,
  onPickFolder,
  onRemoveWorktree,
  onForgetFolder,
  branch = null,
  worktreeOf = null,
  index = null,
  changes = null,
  onOpenChanges,
  branchPicker,
}: Props) {
  const shownBranch = (branchPicker?.worktree && branchPicker.base) || branch || "";
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
            title={branchPicker.worktree ? "The branch a new worktree starts from" : "Switch the folder's branch"}
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
          <button
            type="button"
            role="checkbox"
            className={`folder-tab-worktree${branchPicker.worktree ? " on" : ""}`}
            aria-checked={branchPicker.worktree}
            aria-label="Worktree"
            title={`The first message starts a worktree from ${shownBranch}: its own folder on a new branch, this one left as it is`}
            onClick={() => branchPicker.onWorktree(!branchPicker.worktree)}
          >
            <span className="folder-tab-check" aria-hidden="true">
              {branchPicker.worktree && <Check size={9} strokeWidth={3.5} />}
            </span>
            <GitFork size={12} />
            <span className="folder-tab-narrow-hide">Worktree</span>
          </button>
        </>
      )}
      {path && branch && !branchPicker && (
        <span className="folder-tab-branch" title="Checked-out branch">
          <GitBranch size={12} />
          <span>{branch}</span>
        </span>
      )}
      {path && index && <IndexBadge state={index} />}
      {path && changes && changes.files > 0 && (
        <button
          type="button"
          className="folder-tab-changes"
          title={`${changes.files} ${changes.files === 1 ? "file" : "files"} changed since the last commit`}
          onClick={onOpenChanges}
        >
          <span className="add">+{changes.add}</span>
          <span className="del">−{changes.del}</span>
        </button>
      )}
    </div>
  );
}
