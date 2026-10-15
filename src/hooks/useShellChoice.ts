import { useEffect, useState } from "react";
import { setShell, shellChoices, type ShellChoices, type ShellKind } from "../lib/settings";

/** Settings → Agent → Shell: what is installed and which one runs, read when the pane opens. */
export function useShellChoice() {
  const [choices, setChoices] = useState<ShellChoices | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    shellChoices().then(setChoices, (e) => setError(String(e)));
  }, []);

  const choose = async (kind: ShellKind) => {
    try {
      await setShell(kind);
      setChoices((current) => (current ? { ...current, chosen: kind } : current));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return { choices, error, choose };
}
