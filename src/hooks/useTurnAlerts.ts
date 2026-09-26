import { useEffect, useRef } from "react";
import { ATTENTION_TEXT, attentionFor, notifyInBackground, soundFor, type AlertPrefs } from "../lib/attention";
import type { TurnStatus } from "../lib/chatTurnReducer";
import { playSound, preloadSounds } from "../lib/sounds";

/**
 * Tells the user the agent needs them — it finished, failed, or waits on a
 * card: a sound when a turn ends, and a system notification titled with the
 * chat's name while the window is in the background. Each moment and each way is
 * switched on its own in Settings.
 */
export function useTurnAlerts({
  status,
  failed,
  chatTitle,
  prefs,
}: {
  status: TurnStatus;
  /** Whether the turn that just ended ended in an error. */
  failed: boolean;
  chatTitle: string | null;
  prefs: AlertPrefs;
}) {
  const last = useRef(status);
  const anySound = prefs.sound.done || prefs.sound.failed;

  useEffect(() => {
    if (anySound) preloadSounds();
  }, [anySound]);

  useEffect(() => {
    const attention = attentionFor(last.current, status, failed);
    last.current = status;
    if (!attention) return;
    // Neither may break the chat: no audio device, or no notification
    // permission, only means the user is not told this way.
    const sound = soundFor(attention, prefs);
    if (sound) playSound(sound).catch(() => {});
    if (prefs.notify[attention]) notifyInBackground(chatTitle || "Kibo Agent", ATTENTION_TEXT[attention]).catch(() => {});
  }, [status, failed, chatTitle, prefs]);
}
