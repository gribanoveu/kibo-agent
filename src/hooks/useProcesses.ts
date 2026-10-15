import { useCallback } from "react";
import { useLiveRead } from "./useLiveRead";
import { onProcessChanged, processesList, processesRunning, stopProcess, type ProcessInfo, type ProcessView } from "../lib/chat";

/**
 * The background processes, read while the Terminal tab is open: when it
 * opens, and each time the backend says one of them changed — at most once a
 * tick per process however much it writes.
 */
export function useProcesses(visible: boolean) {
  const { value: processes, setValue: setProcesses, error, setError } = useLiveRead(processesList, onProcessChanged, [] as ProcessView[], visible);

  const stop = useCallback(async (id: number) => {
    try {
      setProcesses(await stopProcess(id));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  return { processes, error, stop };
}

/**
 * The background processes still running, for the chat to count. Listened to
 * for as long as the window is open — the chat always is — and read once when
 * it starts listening.
 */
export function useRunningProcesses() {
  // A count that cannot be read stays as it was: the chat has nowhere to say why.
  return useLiveRead(processesRunning, onProcessChanged, [] as ProcessInfo[]).value;
}
