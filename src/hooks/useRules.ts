import { useCallback, useEffect, useState } from "react";
import { type RuleListItem, rulesList, setRuleEnabled } from "../lib/skills";

/**
 * The open folder's instruction files, re-read when the tab opens or the
 * folder changes: they are edited in the repository, outside the app.
 */
export function useRules(visible: boolean, workspace: string | null) {
  const [rules, setRules] = useState<RuleListItem[]>([]);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setRules(await rulesList());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    if (visible) reload();
  }, [visible, workspace, reload]);

  const setEnabled = useCallback(
    async (path: string, enabled: boolean) => {
      setError(null);
      setRules((list) => list.map((r) => (r.path === path ? { ...r, enabled } : r)));
      try {
        await setRuleEnabled(path, enabled);
      } catch (e) {
        setError(String(e));
      }
      await reload();
    },
    [reload],
  );

  return { rules, error, setEnabled };
}
