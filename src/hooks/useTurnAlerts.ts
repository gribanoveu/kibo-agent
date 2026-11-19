import { useEffect, useRef } from "react";
import { ATTENTION_TEXT, attentionFor, notifyInBackground, soundFor, type AlertPrefs } from "../lib/attention";
import type { TurnStatus } from "../lib/chatTurnReducer";
import { playSound, preloadSounds } from "../lib/sounds";

/** One chat's turn, as the alerts follow it. */
export type TurnAlert = {
  status: TurnStatus;
  /** Whether the turn that just ended ended in an error. */
  failed: boolean;
  /** Messages waiting to be sent: a turn that ends with one is not the end of the work. */
  queued?: number;
  chatTitle: string | null;
};

const statusesOf = (turns: Record<string, TurnAlert>) =>
  Object.fromEntries(Object.entries(turns).map(([key, turn]) => [key, turn.status]));

/**
 * Tells the user the agent needs them — it finished, failed, or waits on a
 * card: a sound when a turn ends, and a system notification titled with the
 * chat's name while the window is in the background. Each moment and each way is
 * switched on its own in Settings.
 *
 * Every chat's turn is followed, by its session key — the one on screen and
 * those left working in another chat or folder (F-8.7). A key not seen before
 * is a chat opened, not a turn that ended: the status it opens with is where
 * it was left.
 */
export function useTurnAlerts({ turns, prefs }: { turns: Record<string, TurnAlert>; prefs: AlertPrefs }) {
  const last = useRef(statusesOf(turns));
  const anySound = prefs.sound.done || prefs.sound.failed;

  useEffect(() => {
    if (anySound) preloadSounds();
  }, [anySound]);

  useEffect(() => {
    const previous = last.current;
    last.current = statusesOf(turns);
    for (const [key, turn] of Object.entries(turns)) {
      const before = previous[key];
      if (before === undefined) continue;
      const attention = attentionFor(before, turn.status, turn.failed);
      if (!attention || (attention === "done" && (turn.queued ?? 0) > 0)) continue;
      // Neither may break the chat: no audio device, or no notification
      // permission, only means the user is not told this way.
      const sound = soundFor(attention, prefs);
      if (sound) playSound(sound).catch(() => {});
      if (prefs.notify[attention]) notifyInBackground(turn.chatTitle || "Kibo Agent", ATTENTION_TEXT[attention]).catch(() => {});
    }
  }, [turns, prefs]);
}
