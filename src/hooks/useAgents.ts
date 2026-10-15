import { useCallback } from "react";
import { useLiveRead } from "./useLiveRead";
import { type AgentInfo, agentsList, onAgentChanged, stopAgent } from "../lib/agents";

/**
 * The helper agents' runs, read while the Agents tab is open: when it opens,
 * and each time the backend says one of them changed.
 */
export function useAgents(visible: boolean) {
  const { value: agents, setValue: setAgents, error, setError } = useLiveRead(agentsList, onAgentChanged, [] as AgentInfo[], visible);

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
