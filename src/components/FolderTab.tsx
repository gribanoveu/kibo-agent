import { Folder, FolderPlus, GitBranch, GitFork, Trash2, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import type { RecentFolder } from "../lib/chat";
import type { BranchRef } from "../lib/git";
import "./FolderTab.css";

/** Not a path: no folder is called this. */
const PICK = "\u0000pick";
/** Not a branch: the row that turns Worktree on or off from the branch menu. */
const WORKTREE = "\u0000worktree";

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
    /** Turns Worktree on/off from the branch menu's own top row. Without it
        the menu only lists branches. */
    onWorktree?: (on: boolean) => void;
    /** Makes a worktree from this branch straight away — the direct gesture,
        no toggle and no base to pick first. */
    onWorktreeFrom?: (name: string) => void;
  };
  /** The chat on screen has started: its folder is its own, shown and not
      chosen here, and its branch is left to Overview — a new chat is where a
      folder and a branch are picked. */
  folderFixed?: boolean;
};

/** Where the next message will be worked on: the folder, and for a new chat its branch. Sits
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
  folderFixed = false,
}: Props) {
  const shownBranch = (branchPicker?.worktree && branchPicker.base) || branch || "";
  const branchShown = path && branch && !folderFixed;
  return (
    <div className={`folder-tab${path ? "" : " no-folder"}`}>
      <Dropdown
        locked={folderFixed && !!path}
        title={
          (folderFixed && path ? `This chat works in ${path}\nA new chat picks its folder` : null) ??
          (path && worktreeOf
            ? `A worktree of ${worktreeOf}\nThis folder: ${path}`
            : path ?? "Choose the folder the agent works in")
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
      {branchShown && branchPicker && (
        <Dropdown
          // Openable even while a turn locks switching: the worktree rows are
          // the way round it, so they must stay reachable; the pick itself is
          // refused upstream with the reason.
          locked={false}
          title={
            (branchPicker.locked && !branchPicker.worktree
              ? branchPicker.locked
              : undefined) ??
            (branchPicker.worktree ? "The branch a new worktree starts from" : "Switch the folder's branch")
          }
          heading={branchPicker.worktree ? "Start a worktree from" : "Branch"}
          note={
            branchPicker.locked ??
            (branchPicker.worktree
              ? "The first message creates the worktree from the base picked here, opens it and is sent there."
              : undefined)
          }
          label={
            <span className="folder-tab-name">
              {branchPicker.worktree ? <GitFork size={12} /> : <GitBranch size={12} />}
              <span className="folder-tab-text">{shownBranch}</span>
            </span>
          }
          value={shownBranch}
          options={[
            // The Worktree switch itself, moved here from the composer's panel:
            // a toggle row above the branches, which become the base while on.
            ...(branchPicker.onWorktree
              ? [
                  {
                    value: WORKTREE,
                    label: "Worktree",
                    hint: branchPicker.worktree ? "on — the branch below is where it starts" : "a new folder on its own branch",
                    checked: branchPicker.worktree,
                    keepOpen: true,
                  },
                ]
              : []),
            ...branchPicker.branches.map((one, i) => ({
              value: one.name,
              hint: one.remote ? "remote" : undefined,
              divided: i === 0 && !!branchPicker.onWorktree,
              // The direct gesture: a worktree from this branch, made now.
              ...(branchPicker.onWorktreeFrom
                ? {
                    action: {
                      icon: <GitFork size={13} />,
                      title: `New worktree from ${one.name}`,
                      onRun: () => branchPicker.onWorktreeFrom!(one.name),
                    },
                  }
                : {}),
            })),
          ]}
          emptyLabel="Reading branches…"
          onOpen={branchPicker.onOpen}
          onPick={(value) =>
            value === WORKTREE ? branchPicker.onWorktree?.(!branchPicker.worktree) : branchPicker.onPick(value)
          }
        />
      )}
      {branchShown && !branchPicker && (
        <span className="folder-tab-branch" title="Checked-out branch">
          <GitBranch size={12} />
          <span>{branch}</span>
        </span>
      )}
    </div>
  );
}
