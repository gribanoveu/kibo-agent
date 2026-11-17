import { useCallback, useRef, useState } from "react";
import { processesList, type ProcessView } from "../lib/processes";
import { terminalList, type TerminalInfo } from "../lib/terminal";
import type { ChatActivity } from "../types";

/** What stands between the user and another folder. */
export type SwitchBlock =
  // A folder cannot close under a turn that works in it (F-8 invariant 5).
  // `chats`: the turns' chats, which Stop ends; `waiting`: one is paused on an
  // approval card, which only an answer ends.
  | { kind: "agent"; folder: string; chats: string[]; waiting: boolean }
  // Closing a folder stops its background processes and terminals (F-8.3);
  // asked, not refused. `folders` are the ones that close.
  | { kind: "processes"; folders: string[]; processes: ProcessView[]; terminals: TerminalInfo[]; go: () => void };

/** The turns working in a folder: each chat's id, and whether it waits on a card. */
export type TurnsIn = (folder: string) => { id: string; waiting: boolean }[];

/**
 * `TurnsIn` from the chats' activity and the folder each is listed under. A
 * chat not listed yet — saved a moment ago — counts in none: which folder it
 * works in the window cannot tell, and the backend, which can, refuses to
 * close that folder or switch its branch under it.
 */
export const turnsIn =
  (activity: Record<string, ChatActivity>, folderOf: (id: string) => string | null): TurnsIn =>
  (folder) =>
    Object.entries(activity)
      .filter(([id, state]) => state !== "finished" && folderOf(id) === folder)
      .map(([id, state]) => ({ id, waiting: state === "awaitingApproval" }));

/**
 * Changing the active folder — opening one, a chat of another one, a new chat
 * in another one — goes ahead whatever runs: a turn keeps its own folder
 * (F-8.7). Closing a folder ends what runs in it: a turn there has to end
 * first, and before processes and terminals stop the user is told — the
 * folders in `closing` have theirs read then, once, from the backend.
 */
export function useFolderSwitch(turnsIn: TurnsIn) {
  const [blocked, setBlocked] = useState<SwitchBlock | null>(null);
  // What to do if the dialog closes without the switch: a message on its way
  // to the other folder goes back to the box. Cleared by the switch itself.
  const onCancelled = useRef<(() => void) | null>(null);

  const guard = useCallback(
    async (go: () => void, onCancel?: () => void, closing: string[] = []) => {
      if (closing.length === 0) return go();
      for (const folder of closing) {
        const turns = turnsIn(folder);
        if (turns.length === 0) continue;
        onCancel?.();
        return setBlocked({ kind: "agent", folder, chats: turns.map((t) => t.id), waiting: turns.some((t) => t.waiting) });
      }
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
    [turnsIn],
  );

  const close = useCallback(() => {
    onCancelled.current?.();
    onCancelled.current = null;
    setBlocked(null);
  }, []);
  return { blocked, guard, close };
}
