import { useEffect, useRef } from "react";

type Agent = {
  turn: { blocks: readonly unknown[] };
  open: (id: string) => unknown;
  reset: () => void;
};

/**
 * Which conversation the thread shows as the folder changes.
 *
 * At launch the app comes back where it was: in the folder it reopened, the
 * conversation touched last (`latest` — the list is newest first). Once;
 * after that an empty thread is the user's choice.
 *
 * On a switch the thread starts fresh. A chat is saved under the folder open
 * at the time, so one carried across a switch would move there; left behind,
 * it stays in the old folder's list.
 */
export function useFolderConversation(
  path: string | null,
  resumed: boolean,
  latest: string | undefined,
  agent: Agent,
  moved?: { id: string; path: string } | null,
  onMoved?: () => void,
) {
  const reopened = useRef(false);
  useEffect(() => {
    if (reopened.current || !resumed || !latest) return;
    reopened.current = true;
    if (agent.turn.blocks.length === 0) void agent.open(latest);
  }, [resumed, latest, agent]);

  const shown = useRef(path);
  useEffect(() => {
    if (shown.current && path !== shown.current) agent.reset();
    shown.current = path;
    // A moved chat is reopened only once its worktree is actually open; doing
    // it after the invoke alone would race the folder-change reset above.
    if (moved?.path === path) void Promise.resolve(agent.open(moved.id)).then(onMoved);
  }, [path, agent, moved, onMoved]);
}
