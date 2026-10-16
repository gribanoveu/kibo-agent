import { useEffect, useState } from "react";
import { currentBranch, worktreeOf } from "../lib/chat";
import { onGitChanged } from "../lib/git";

/**
 * The branch checked out in the open folder, for the chat header. Read again
 * when the folder changes, whenever a turn ends — the agent can switch
 * branches itself, with a command — and when the repository says HEAD moved:
 * a switch from the branch menu, or one made in a terminal.
 */
export function useGitBranch(workspace: string | null, turnStatus: string) {
  const [branch, setBranch] = useState<string | null>(null);

  useEffect(() => {
    if (!workspace || turnStatus === "running") return;
    let live = true;
    const load = () =>
      currentBranch()
        .then((name) => live && setBranch(name))
        // A branch that cannot be read shows the folder instead; not worth a message.
        .catch(() => live && setBranch(null));
    void load();
    let stop: (() => void) | null = null;
    onGitChanged((root) => root === workspace && void load()).then((off) => (live ? (stop = off) : off()));
    return () => {
      live = false;
      stop?.();
    };
  }, [workspace, turnStatus]);

  return workspace ? branch : null;
}

/**
 * The main working tree when the open folder is a worktree of it. Read once
 * per folder: whether a folder is a worktree does not change while it is open.
 */
export function useWorktreeOf(workspace: string | null) {
  const [main, setMain] = useState<string | null>(null);

  useEffect(() => {
    setMain(null);
    if (!workspace) return;
    let live = true;
    worktreeOf()
      .then((path) => live && setMain(path))
      .catch(() => live && setMain(null));
    return () => {
      live = false;
    };
  }, [workspace]);

  return main;
}
