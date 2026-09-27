import { useCallback, useRef, useState } from "react";
import { processesList, type ProcessView } from "../lib/chat";
import { terminalList, type TerminalInfo } from "../lib/terminal";

/** What stands between the user and another folder. */
export type SwitchBlock =
  // The turn saves its chat into the folder open when it ends: switching
  // under it would file the chat in the other one. So this one is refused.
  // `waiting`: paused on an approval card, which only an answer ends.
  | { kind: "agent"; waiting: boolean }
  // `workspace_open` stops every background process and closes every
  // terminal the user has open; asked, not refused.
  | { kind: "processes"; processes: ProcessView[]; terminals: TerminalInfo[]; go: () => void };

/**
 * Switching folders ends what runs in the open one. Before it does, the user
 * is told — and the processes and terminals are read then, once, from the backend.
 */
export function useFolderSwitch(agent: "running" | "waiting" | null) {
  const [blocked, setBlocked] = useState<SwitchBlock | null>(null);
  // What to do if the dialog closes without the switch: a message on its way
  // to the other folder goes back to the box. Cleared by the switch itself.
  const onCancelled = useRef<(() => void) | null>(null);

  const guard = useCallback(
    async (go: () => void, onCancel?: () => void) => {
      if (agent) {
        onCancel?.();
        return setBlocked({ kind: "agent", waiting: agent === "waiting" });
      }
      const [processes, terminals] = await Promise.all([
        processesList().catch(() => []),
        terminalList().catch(() => []),
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
