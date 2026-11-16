import { useCallback, useRef, useState } from "react";
import { listWorkspaces } from "../lib/chat";
import { processesList, type ProcessView } from "../lib/processes";
import { terminalList, type TerminalInfo } from "../lib/terminal";

/** What stands between the user and another folder. */
export type SwitchBlock =
  // The turn saves its chat into the folder open when it ends: switching
  // under it would file the chat in the other one. So this one is refused.
  // `waiting`: paused on an approval card, which only an answer ends.
  | { kind: "agent"; waiting: boolean }
  // `workspace_open` closes every other open folder: their background
  // processes stop and their terminals close (F-8.3); asked, not refused.
  | { kind: "processes"; processes: ProcessView[]; terminals: TerminalInfo[]; go: () => void };

/**
 * Switching folders ends what runs in every other open folder — the active one, and one a chat
 * moved to its worktree left running. Before it does, the user is told — and those folders'
 * processes and terminals are read then, once, from the backend. `target`, when known, is the
 * folder being opened: what runs there goes on.
 */
export function useFolderSwitch(agent: "running" | "waiting" | null) {
  const [blocked, setBlocked] = useState<SwitchBlock | null>(null);
  // What to do if the dialog closes without the switch: a message on its way
  // to the other folder goes back to the box. Cleared by the switch itself.
  const onCancelled = useRef<(() => void) | null>(null);

  const guard = useCallback(
    async (go: () => void, onCancel?: () => void, target?: string) => {
      if (agent) {
        onCancel?.();
        return setBlocked({ kind: "agent", waiting: agent === "waiting" });
      }
      const closing = (await listWorkspaces().catch(() => [])).map((f) => f.path).filter((path) => path !== target);
      const [processes, terminals] = await Promise.all([
        Promise.all(closing.map((path) => processesList(null, path).catch(() => []))).then((lists) => lists.flat()),
        Promise.all(closing.map((path) => terminalList(path).catch(() => []))).then((lists) => lists.flat()),
      ]);
      const running = processes.filter((p) => p.state.state === "running");
      const shells = terminals.filter((t) => t.state.state === "running");
      if (running.length === 0 && shells.length === 0) return go();
      onCancelled.current = onCancel ?? null;
      const proceed = () => {
        onCancelled.current = null;
        go();
      };
      setBlocked({ kind: "processes", processes: running, terminals: shells, go: proceed });
    },
    [agent],
  );

  const close = useCallback(() => {
    onCancelled.current?.();
    onCancelled.current = null;
    setBlocked(null);
  }, []);
  return { blocked, guard, close };
}
