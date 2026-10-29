import { useCallback } from "react";
import { useLiveRead } from "./useLiveRead";
import { type AgentInfo, agentsList, onAgentChanged, stopAgent } from "../lib/agents";

/**
 * The helper agents' runs the open chat started, read while the Agents tab is
 * open: when it opens, when another chat is opened, and each time the backend
 * says one of them changed. A chat not started yet has none.
 */
export function useAgents(visible: boolean, chatId: string | null) {
  const read = () => (chatId ? agentsList(chatId) : Promise.resolve([])).then((list) => ({ chatId, list }));
  const { value, setValue, error, setError } = useLiveRead(read, onAgentChanged, { chatId: null as string | null, list: [] as AgentInfo[] }, visible, chatId ?? "");
  // The last chat's list stays read until this one's arrives: it is not this one's to show.
  const agents = value.chatId === chatId ? value.list : [];

  const stop = useCallback(
    async (id: number) => {
      // Stop is on a listed row, and only a chat's own are listed.
      if (!chatId) return;
      try {
        setValue({ chatId, list: await stopAgent(id, chatId) });
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [chatId],
  );

  return { agents, error, stop };
}
