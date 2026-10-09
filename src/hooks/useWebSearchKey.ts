import { useEffect, useState } from "react";
import {
  saveWebSearchKey,
  setWebSearchBackend,
  webSearchBackend,
  webSearchKeyStatus,
  webSearchUsage,
  type WebSearchBackend,
  type WebUsage,
} from "../lib/chat";

/**
 * Settings → Web search: which search a chat uses, whether a Tavily key is
 * saved and what it has spent — read when the pane opens and again after a
 * save, which is also how a mistyped key is found out.
 */
export function useWebSearchKey() {
  const [backend, setBackend] = useState<WebSearchBackend | null>(null);
  const [hasKey, setHasKey] = useState(false);
  const [usage, setUsage] = useState<WebUsage | null>(null);
  const [error, setError] = useState<string | null>(null);

  const readUsage = () =>
    webSearchUsage().then(
      (read) => {
        setUsage(read);
        setError(null);
      },
      (e) => {
        setUsage(null);
        setError(String(e));
      },
    );

  useEffect(() => {
    webSearchBackend().then(setBackend, (e) => setError(String(e)));
    webSearchKeyStatus().then(setHasKey, () => {});
    void readUsage();
  }, []);

  /** Saves `key`, or deletes the stored one when it is empty. */
  const save = async (key: string): Promise<boolean> => {
    try {
      await saveWebSearchKey(key);
      setHasKey(key.trim() !== "");
      setError(null);
      await readUsage();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  /** From the next turn on: the tools are given, or not, per request. */
  const choose = async (next: WebSearchBackend) => {
    try {
      await setWebSearchBackend(next);
      setBackend(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return { backend, choose, hasKey, usage, error, save };
}
