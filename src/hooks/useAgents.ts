import { useCallback, useEffect, useState } from "react";
import { agentsList, onAgentChanged, stopAgent, type AgentInfo } from "../lib/chat";

/**
 * The helper agents' runs, read while the Agents tab is open: when it opens,
 * and each time the backend says one of them changed.
 */
export function useAgents(visible: boolean) {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!visible) return;
    let live = true;
    const load = () =>
      agentsList().then(
        (list) => {
          if (!live) return;
          setAgents(list);
          setError(null);
        },
        (e) => live && setError(String(e)),
      );
    load();
    let unlisten: (() => void) | undefined;
    onAgentChanged(() => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [visible]);

  const stop = useCallback(async (id: number) => {
    try {
      setAgents(await stopAgent(id));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  return { agents, error, stop };
}
