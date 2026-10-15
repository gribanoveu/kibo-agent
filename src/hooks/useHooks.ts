import { useCallback, useEffect, useState } from "react";
import { hooksConfig, type HooksView, saveHooksConfig } from "../lib/agentHooks";

/** The hooks file, re-read whenever the tab or the editor opens: it is the user's, and may change outside the app. */
export function useHooks(visible: boolean) {
  const [view, setView] = useState<HooksView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!visible) return;
    hooksConfig().then(
      (v) => {
        setView(v);
        setError(null);
      },
      (e) => setError(String(e)),
    );
  }, [visible]);

  /** Resolves to whether it was stored; a refusal is left in `error` for the editor to show. */
  const save = useCallback(async (text: string) => {
    try {
      setView(await saveHooksConfig(text));
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  }, []);

  return { view, error, save };
}
