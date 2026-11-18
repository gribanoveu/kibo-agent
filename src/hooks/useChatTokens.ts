import { useEffect, useState } from "react";
import { chatTokens, subagentTokens } from "../lib/usage";

/**
 * A token total of the chat `chatId`, read while `shown`, and again whenever
 * one of `changed` does: a round's usage, a turn coming to rest, a title or a
 * summary that has come back. Only the latest read is drawn, so an earlier one
 * landing late cannot put back a smaller total, or the chat left's. `null`
 * until read, and for no chat.
 */
function useTokens(read: (chatId: string) => Promise<number>, chatId: string | null, shown: boolean, changed: unknown[]): number | null {
  const [spent, setSpent] = useState<{ chatId: string; tokens: number | null } | null>(null);
  useEffect(() => {
    if (!shown || chatId === null) return;
    let live = true;
    read(chatId).then(
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

/** What the chat has spent on the model, every call of it — the session total. */
export function useChatTokens(chatId: string | null, shown: boolean, ...changed: unknown[]): number | null {
  return useTokens(chatTokens, chatId, shown, changed);
}

/** The subagents' share of the session total — the Overview's row under it. */
export function useSubagentTokens(chatId: string | null, shown: boolean, ...changed: unknown[]): number | null {
  return useTokens(subagentTokens, chatId, shown, changed);
}