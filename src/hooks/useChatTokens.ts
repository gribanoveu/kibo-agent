import { useEffect, useState } from "react";
import { chatTokens } from "../lib/usage";

/**
 * What the chat `chatId` has spent on the model, in tokens — read while
 * `shown`, and again whenever one of `changed` does: a round's usage, a turn
 * coming to rest, a title or a summary that has come back. Only the latest
 * read is drawn, so an earlier one landing late cannot put back a smaller
 * total, or the chat left's. `null` until read, and for no chat.
 */
export function useChatTokens(chatId: string | null, shown: boolean, ...changed: unknown[]): number | null {
  const [spent, setSpent] = useState<{ chatId: string; tokens: number | null } | null>(null);
  useEffect(() => {
    if (!shown || chatId === null) return;
    let live = true;
    chatTokens(chatId).then(
      (tokens) => live && setSpent({ chatId, tokens }),
      () => live && setSpent({ chatId, tokens: null }),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatId, shown, ...changed]);
  return spent?.chatId === chatId ? spent.tokens : null;
}
