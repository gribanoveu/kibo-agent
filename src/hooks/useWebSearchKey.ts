import { useEffect, useState } from "react";
import { saveWebSearchKey, webSearchKeyStatus } from "../lib/chat";

/** Settings → Web search: whether a key is saved, read when the pane opens. */
export function useWebSearchKey() {
  const [hasKey, setHasKey] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    webSearchKeyStatus().then(setHasKey, () => {});
  }, []);

  /** Saves `key`, or deletes the stored one when it is empty. */
  const save = async (key: string): Promise<boolean> => {
    try {
      await saveWebSearchKey(key);
      setHasKey(key.trim() !== "");
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  return { hasKey, error, save };
}
