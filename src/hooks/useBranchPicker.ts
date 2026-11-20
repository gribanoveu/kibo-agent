import { useCallback, useEffect, useState } from "react";
import { type BranchRef, gitBranches, gitCheckout, gitWorktreeAdd } from "../lib/git";
import type { ImagePart } from "../lib/images";

/** A switch refused because it would overwrite these files; held until the user decides. */
export type BranchConflict = { branch: string; paths: string[] };

type Deps = {
  /** The chat on screen. Accepted from callers; the picker keeps no per-chat
      mode now that a worktree is an explicit action, not a tick to remember. */
  chatId: string | null;
  /** The folder the branches are read from and switched in — the active one. */
  workspace: string | null;
  /** The branch checked out now: picking it again is not a switch. */
  current: string | null;
  /** Runs `go` once leaving the open folder is agreed to, `onCancel` if it is not — `useFolderSwitch`'s guard. */
  guard: (go: () => void, onCancel?: () => void) => void;
  open: (path: string) => Promise<boolean>;
  notify: (message: string) => void;
  /** Sends a message in the folder open now — the agent's; `sent` as `useAgentTurn`'s. */
  send: (text: string, sent?: string, images?: ImagePart[]) => void;
  /** The open folder is on another branch now — the chat keeps it as its own. */
  onSwitched?: () => void;
};

/**
 * The branch a new chat starts on: picking a branch switches the open folder.
 * A worktree is made from a branch by an explicit action (`startWorktree`),
 * which opens it and can send a first message there — no deferred mode.
 *
 * Call it after `useFolderConversation`: the message is sent from an effect,
 * and the fresh thread the folder switch starts has to be in place first.
 *
 * The app never decides what happens to uncommitted work. A switch that would
 * overwrite it is refused by the backend and shown as a conflict, with a
 * worktree as the way round it; committing or stashing is left to the user,
 * or to the agent when asked.
 */
export function useBranchPicker({ workspace, current, guard, open, notify, send, onSwitched }: Deps) {
  const [branches, setBranches] = useState<BranchRef[]>([]);
  const [conflict, setConflict] = useState<BranchConflict | null>(null);
  // A first message waiting for its worktree to be the open folder.
  const [waiting, setWaiting] = useState<{ text: string; sent?: string; images?: ImagePart[] } | null>(null);

  useEffect(() => {
    if (waiting === null) return;
    setWaiting(null);
    send(waiting.text, waiting.sent, waiting.images);
    // Once per message: `send` is a new function every render.
  }, [waiting]);

  // Read as the menu opens rather than kept current: a branch made in a
  // terminal is then there the next time anyone looks.
  const load = useCallback(() => {
    gitBranches(workspace).then(setBranches, (e) => notify(String(e)));
  }, [notify, workspace]);

  /**
   * Makes a worktree from `from`, opens it, and sends `message` there;
   * `false` when it did not open — cancelled or refused — and nothing was
   * sent. Made only after leaving the folder is agreed to: a worktree nobody
   * opens would be left on disk for nothing.
   */
  const startWorktree = useCallback(
    (from: string, message?: string, sent?: string, images?: ImagePart[]) =>
      new Promise<boolean>((resolve) => {
        setConflict(null);
        guard(
          () => {
            void (async () => {
              try {
                const opened = await open(await gitWorktreeAdd(from, workspace));
                if (opened) {
                  if (message !== undefined) setWaiting({ text: message, sent, images });
                }
                resolve(opened);
              } catch (e) {
                notify(String(e));
                resolve(false);
              }
            })();
          },
          () => resolve(false),
        );
      }),
    [guard, open, notify, workspace],
  );

  const pick = useCallback(
    async (name: string) => {
      if (name === current) return;
      try {
        const outcome = await gitCheckout(name, workspace);
        if (outcome.kind === "conflicts") setConflict({ branch: name, paths: outcome.paths });
        else onSwitched?.();
      } catch (e) {
        notify(String(e));
      }
    },
    [current, notify, onSwitched, workspace],
  );

  const closeConflict = useCallback(() => setConflict(null), []);

  return {
    branches,
    load,
    pick,
    conflict,
    closeConflict,
    startWorktree,
  };
}
