import { useCallback, useEffect, useState } from "react";
import { setSkillEnabled, setSkillSourceEnabled, skillsList, type SkillSourceItem, type SkillsView } from "../lib/skills";

/**
 * The skills — the open folder's and the user's — re-read whenever the tab
 * showing them opens or another folder is opened: skills are files edited
 * outside the app, so a list read once at start-up would go stale without
 * anyone noticing.
 */
export function useSkills(visible: boolean, workspace: string | null = null) {
  const [view, setView] = useState<SkillsView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setView(await skillsList());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    if (visible) reload();
  }, [visible, workspace, reload]);

  const setEnabled = useCallback(
    async (name: string, enabled: boolean) => {
      // Shown at once, on every row of the name — the switch is by name; the
      // reload that follows is what the file now says.
      setError(null);
      setView((v) => v && { ...v, skills: v.skills.map((s) => (s.name === name ? { ...s, enabled } : s)) });
      try {
        await setSkillEnabled(name, enabled);
      } catch (e) {
        setError(String(e));
      }
      await reload();
    },
    [reload],
  );

  /** A whole folder on or off, from Settings; the list is read again, since its skills come and go. */
  const setSourceEnabled = useCallback(
    async (id: SkillSourceItem["id"], enabled: boolean) => {
      setError(null);
      setView((v) => v && { ...v, sources: v.sources.map((s) => (s.id === id ? { ...s, enabled } : s)) });
      try {
        await setSkillSourceEnabled(id, enabled);
      } catch (e) {
        setError(String(e));
      }
      await reload();
    },
    [reload],
  );

  return { view, error, setEnabled, setSourceEnabled };
}
