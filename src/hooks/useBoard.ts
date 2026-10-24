import { useCallback } from "react";
import { useLiveRead } from "./useLiveRead";
import { boardChange, boardList, onBoardChanged, type BoardChange, type Card } from "../lib/board";

/**
 * The board, read while its page is open: when it opens, and each time the
 * backend says it changed — a card the model just added shows up on its own.
 */
export function useBoard() {
  const { value: cards, setValue: setCards, error, setError } = useLiveRead(boardList, onBoardChanged, [] as Card[]);

  /** Whether it was kept: a form stays open on a refusal, which `error` says. */
  const change = useCallback(async (next: BoardChange) => {
    try {
      setCards(await boardChange(next));
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  }, []);

  return { cards, error, change };
}
