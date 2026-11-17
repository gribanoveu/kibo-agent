import { useCallback, useRef, useState } from "react";
import { processesList, type ProcessView } from "../lib/processes";
import { terminalList, type TerminalInfo } from "../lib/terminal";

/** What stands between the user and another folder. */
export type SwitchBlock =
  // The turn saves its chat into the active folder when it ends: changing it
  // under the turn would file the chat in the other one. So this one is
  // refused, until F-8.7 lets turns run while the window moves.
  // `waiting`: paused on an approval card, which only an answer ends.
  | { kind: "agent"; waiting: boolean }
  // Closing a folder stops its background processes and terminals (F-8.3);
  // asked, not refused. `folders` are the ones that close.
  | { kind: "processes"; folders: string[]; processes: ProcessView[]; terminals: TerminalInfo[]; go: () => void };

/**
 * Changing the active folder — opening one, a chat of another one, a new chat
 * in another one, closing the active one — waits for every turn to finish. Closing a folder ends what
 * runs in it: before it does, the user is told, and the folders in `closing`
 * have their processes and terminals read then, once, from the backend.
 * Opening a folder closes none (F-8.6).
 */
export function useFolderSwitch(agent: "running" | "waiting" | null, active: string | null = null) {
  const [blocked, setBlocked] = useState<SwitchBlock | null>(null);
  // What to do if the dialog closes without the switch: a message on its way
  // to the other folder goes back to the box. Cleared by the switch itself.
  const onCancelled = useRef<(() => void) | null>(null);

  const guard = useCallback(
    async (go: () => void, onCancel?: () => void, closing: string[] = []) => {
      // Closing another folder leaves the active one where it is: a turn
      // does not stand in its way (one in that folder, the backend refuses).
      const moves = closing.length === 0 || (active !== null && closing.includes(active));
      if (agent && moves) {
        onCancel?.();
        return setBlocked({ kind: "agent", waiting: agent === "waiting" });
      }
      if (closing.length === 0) return go();
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
      setBlocked({ kind: "processes", folders: closing, processes: running, terminals: shells, go: proceed });
    },
    [agent, active],
  );

  const close = useCallback(() => {
    onCancelled.current?.();
    onCancelled.current = null;
    setBlocked(null);
  }, []);
  return { blocked, guard, close };
}
