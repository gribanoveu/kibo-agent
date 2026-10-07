import { useCallback, useEffect, useState } from "react";
import { gitBranches, gitCheckout, gitWorktreeAdd, type BranchRef, type ImagePart } from "../lib/chat";

/** A switch refused because it would overwrite these files; held until the user decides. */
export type BranchConflict = { branch: string; paths: string[] };

type Deps = {
  /** The branch checked out now: picking it again is not a switch. */
  current: string | null;
  /** Runs `go` once leaving the open folder is agreed to, `onCancel` if it is not — `useFolderSwitch`'s guard. */
  guard: (go: () => void, onCancel?: () => void) => void;
  open: (path: string) => Promise<boolean>;
  notify: (message: string) => void;
  /** Sends a message in the folder open now — the agent's; `sent` as `useAgentTurn`'s. */
  send: (text: string, sent?: string, images?: ImagePart[]) => void;
};

/**
 * The branch a new chat starts on. With Worktree off, picking a branch
 * switches the open folder. With it on, the pick is only where the worktree
 * will start (`base`); the first message makes it, opens it and is sent there.
 *
 * Call it after `useFolderConversation`: the message is sent from an effect,
 * and the fresh thread the folder switch starts has to be in place first.
 *
 * The app never decides what happens to uncommitted work. A switch that would
 * overwrite it is refused by the backend and shown as a conflict, with a
 * worktree as the way round it; committing or stashing is left to the user,
 * or to the agent when asked.
 */
export function useBranchPicker({ current, guard, open, notify, send }: Deps) {
  const [branches, setBranches] = useState<BranchRef[]>([]);
  const [worktree, setWorktree] = useState(false);
  // Where a worktree starts; `null` is the branch checked out now.
  const [base, setBase] = useState<string | null>(null);
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
    gitBranches().then(setBranches, (e) => notify(String(e)));
  }, [notify]);

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
                const opened = await open(await gitWorktreeAdd(from));
                if (opened) {
                  setWorktree(false);
                  setBase(null);
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
    [guard, open, notify],
  );

  const pick = useCallback(
    async (name: string) => {
      if (worktree) return setBase(name === current ? null : name);
      if (name === current) return;
      try {
        const outcome = await gitCheckout(name);
        if (outcome.kind === "conflicts") setConflict({ branch: name, paths: outcome.paths });
      } catch (e) {
        notify(String(e));
      }
    },
    [worktree, current, notify],
  );

  const closeConflict = useCallback(() => setConflict(null), []);
  // Off, a base chosen earlier is forgotten: on again starts from where the folder is.
  const toggleWorktree = useCallback((on: boolean) => {
    setWorktree(on);
    if (!on) setBase(null);
  }, []);

  return {
    branches,
    load,
    pick,
    worktree,
    setWorktree: toggleWorktree,
    base: base ?? current,
    conflict,
    closeConflict,
    startWorktree,
  };
}
