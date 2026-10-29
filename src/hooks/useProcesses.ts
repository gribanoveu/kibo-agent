import { useCallback } from "react";
import { useLiveRead } from "./useLiveRead";
import { onProcessChanged, processesList, processesRunning, type ProcessInfo, type ProcessView, stopProcess } from "../lib/processes";

/**
 * The background processes the open chat started, read while the Terminal
 * tab is open: when it opens, when another chat is opened, and each time the
 * backend says one of them changed — at most once a tick per process however
 * much it writes. A chat not started yet has none.
 */
export function useProcesses(visible: boolean, chatId: string | null) {
  const read = () => (chatId ? processesList(chatId) : Promise.resolve([])).then((list) => ({ chatId, list }));
  const { value, setValue, error, setError } = useLiveRead(read, onProcessChanged, { chatId: null as string | null, list: [] as ProcessView[] }, visible, chatId ?? "");
  // The last chat's list stays read until this one's arrives: it is not this one's to show.
  const processes = value.chatId === chatId ? value.list : [];

  const stop = useCallback(
    async (id: number) => {
      // Stop is on a listed row, and only a chat's own are listed.
      if (!chatId) return;
      try {
        setValue({ chatId, list: await stopProcess(id, chatId) });
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [chatId],
  );

  return { processes, error, stop };
}

/**
 * The open chat's background processes still running, for the chat to count.
 * Listened to for as long as the window is open — the chat always is — and
 * read once when it starts listening and whenever another chat is opened.
 */
export function useRunningProcesses(chatId: string | null) {
  const read = () => (chatId ? processesRunning(chatId) : Promise.resolve([])).then((list) => ({ chatId, list }));
  // A count that cannot be read stays as it was: the chat has nowhere to say why.
  const { value } = useLiveRead(read, onProcessChanged, { chatId: null as string | null, list: [] as ProcessInfo[] }, true, chatId ?? "");
  return value.chatId === chatId ? value.list : [];
}
