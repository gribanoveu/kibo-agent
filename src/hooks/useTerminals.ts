import { useCallback, useEffect, useState } from "react";
import {
  onTerminalChanged,
  terminalClose,
  terminalList,
  terminalOpen,
  type TerminalInfo,
} from "../lib/terminal";

/**
 * The shells of the folder `workspace`, read while the Terminal tab is open: when it opens,
 * when the folder changes, and each time the backend says one opened, ended or closed.
 */
export function useTerminals(visible: boolean, workspace: string | null) {
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  // Read at least once since the tab opened: an empty list means none, not "not yet".
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!visible) return;
    let live = true;
    const load = () =>
      terminalList(workspace).then(
        (list) => {
          if (!live) return;
          setTerminals(list);
          setLoaded(true);
        },
        (e) => live && setError(String(e)),
      );
    load();
    let unlisten: (() => void) | undefined;
    onTerminalChanged(() => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      setLoaded(false);
      // Another folder's shells are not this one's to draw while its list is read.
      setTerminals([]);
      unlisten?.();
    };
  }, [visible, workspace]);

  /** In the folder `workspace` names. The new one, or null when it did not start — the reason is in `error`. */
  const open = useCallback(async (): Promise<TerminalInfo | null> => {
    try {
      const terminal = await terminalOpen(workspace);
      setTerminals((list) => (list.some((t) => t.id === terminal.id) ? list : [...list, terminal]));
      setError(null);
      return terminal;
    } catch (e) {
      setError(String(e));
      return null;
    }
  }, [workspace]);

  const close = useCallback(async (id: number) => {
    try {
      await terminalClose(id);
      setTerminals((list) => list.filter((t) => t.id !== id));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  return { terminals, loaded, error, open, close };
}
