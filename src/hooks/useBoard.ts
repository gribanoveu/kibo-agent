import { useCallback, useEffect, useState } from "react";
import { boardChange, boardList, onBoardChanged, type BoardChange, type Card } from "../lib/board";

/**
 * The board, read while its page is open: when it opens, and each time the
 * backend says it changed — a card the model just added shows up on its own.
 */
export function useBoard() {
  const [cards, setCards] = useState<Card[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const load = () =>
      boardList().then(
        (list) => {
          if (!live) return;
          setCards(list);
          setError(null);
        },
        (e) => live && setError(String(e)),
      );
    load();
    let unlisten: (() => void) | undefined;
    onBoardChanged(() => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  const change = useCallback(async (next: BoardChange) => {
    try {
      setCards(await boardChange(next));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  return { cards, error, change };
}
