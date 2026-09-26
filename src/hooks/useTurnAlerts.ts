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
  queued = 0,
  chatTitle,
  prefs,
}: {
  status: TurnStatus;
  /** Whether the turn that just ended ended in an error. */
  failed: boolean;
  /** Messages waiting to be sent: a turn that ends with one is not the end of the work. */
  queued?: number;
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
    if (!attention || (attention === "done" && queued > 0)) return;
    // Neither may break the chat: no audio device, or no notification
    // permission, only means the user is not told this way.
    const sound = soundFor(attention, prefs);
    if (sound) playSound(sound).catch(() => {});
    if (prefs.notify[attention]) notifyInBackground(chatTitle || "Kibo Agent", ATTENTION_TEXT[attention]).catch(() => {});
  }, [status, failed, queued, chatTitle, prefs]);
}
