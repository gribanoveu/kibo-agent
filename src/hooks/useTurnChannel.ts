import { useCallback, useEffect, useRef, type Dispatch, type SetStateAction } from "react";
import { onTurnEvent } from "../lib/chat";
import { acceptEvent, compactionEnded, compactionStarted, type TurnState } from "../lib/chatTurnReducer";

/**
 * The window's end of `chat:turn-event`, as the agent's chat and Chat mode
 * both use it: one turn listened to at a time, its events folded into the
 * transcript as they arrive; and a compaction pass's start, said on an id of
 * its own. `model` is the one turns run on — `acceptEvent` says when a cache
 * miss came with a switch. Stops listening when the window part goes.
 */
export function useTurnChannel(setTurn: Dispatch<SetStateAction<TurnState>>, model: string | undefined) {
  const subscribed = useRef<(() => void) | null>(null);
  const modelRef = useRef(model);
  modelRef.current = model;
  useEffect(() => () => subscribed.current?.(), []);

  /** Listens to turn `id`, in place of the one before. */
  const listen = useCallback(
    async (id: string) => {
      subscribed.current?.();
      subscribed.current = await onTurnEvent(id, (event) => {
        // Stamped on arrival: the updater may run renders later, and the speed is timed from these.
        const now = Date.now();
        setTurn((state) => acceptEvent(state, event, now, modelRef.current));
      });
    },
    [setTurn],
  );

  /** Stops listening: the turn ended, or is left. */
  const stop = useCallback(() => {
    subscribed.current?.();
    subscribed.current = null;
  }, []);

  /**
   * Shows a compaction pass under `id` while it runs: the backend says when a
   * summary is being made, which only it knows. Call what this resolves to
   * with the pass's result when it returns — a start arriving after that is
   * stale, not a second pass.
   */
  const watchCompaction = useCallback(
    async (id: string) => {
      let ended = false;
      const off = await onTurnEvent(id, (event) => {
        if (event.type === "historyCompacting" && !ended) setTurn(compactionStarted);
      });
      return (result: { folded: number } | null) => {
        ended = true;
        off();
        setTurn((state) => compactionEnded(state, result));
      };
    },
    [setTurn],
  );

  return { listen, stop, watchCompaction };
}
