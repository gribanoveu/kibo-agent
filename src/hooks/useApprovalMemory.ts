import { useEffect, useRef, useState } from "react";
import { getRememberScope, type RememberScope, restoreUnattended, setRememberScope, setUnattended } from "../lib/chat";

/**
 * The composer's Ask/Auto, remembered per chat or per folder (Settings says
 * which), and put back whenever the chat, the folder or that choice changes.
 *
 * The backend holds and enforces it; this follows. `onRestoredAuto` is told
 * when Auto comes back on by itself — the next write happens without anyone
 * being asked, which is worth saying even when it was chosen long ago.
 */
export function useApprovalMemory(chatId: string | null, folder: string | null, onRestoredAuto: () => void) {
  const [unattended, setValue] = useState(false);
  const [remember, setRemember] = useState<RememberScope>("chat");
  // Read by the restore, which must not re-run each time the value moves.
  const current = useRef(false);
  // Bumped by every restore and pick: a restore answered after the user
  // picked, or after the chat changed again, is stale and dropped.
  const seq = useRef(0);
  const adopt = (auto: boolean) => {
    current.current = auto;
    setValue(auto);
  };

  useEffect(() => {
    getRememberScope().then(setRemember, () => {});
  }, []);

  useEffect(() => {
    const mine = ++seq.current;
    restoreUnattended(chatId).then(
      (auto) => {
        if (mine !== seq.current) return;
        if (auto && !current.current) onRestoredAuto();
        adopt(auto);
      },
      () => {},
    );
  }, [chatId, folder, remember]);

  /** Moves after the backend agreed; the reason on failure. */
  const pick = async (next: boolean): Promise<string | null> => {
    seq.current++;
    try {
      await setUnattended(next, chatId);
      adopt(next);
      return null;
    } catch (e) {
      return String(e);
    }
  };

  const pickRemember = async (next: RememberScope): Promise<string | null> => {
    try {
      await setRememberScope(next);
      setRemember(next);
      return null;
    } catch (e) {
      return String(e);
    }
  };

  return { unattended, pick, remember, pickRemember };
}
