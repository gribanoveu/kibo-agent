import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { inTauri } from "./chat";
import type { TurnStatus } from "./chatTurnReducer";
import type { Sound } from "./sounds";

/** A moment the user is wanted back: the work is done, it failed, or a card is waiting. */
export type Attention = "done" | "failed" | "approval";

/**
 * What a change of the turn's status asks of the user, if anything. Only an
 * end the agent reached counts: a turn the user stopped, or a chat opened
 * (idle to done), is not news to them.
 */
export function attentionFor(prev: TurnStatus, next: TurnStatus, failed: boolean): Attention | null {
  if (prev === next) return null;
  if (next === "awaitingApproval") return "approval";
  if (prev === "running" && next === "done") return failed ? "failed" : "done";
  return null;
}

export const ATTENTION_TEXT: Record<Attention, string> = {
  done: "The agent has finished.",
  failed: "The turn failed.",
  approval: "The agent is waiting for your approval.",
};

/**
 * Which moments call the user back, and how — each switched on its own in
 * Settings. There is no sound for a card: in Ask one comes with every step
 * that writes, and a sound for each is noise.
 */
export type AlertPrefs = {
  sound: { done: boolean; failed: boolean };
  notify: Record<Attention, boolean>;
};

export const DEFAULT_ALERTS: AlertPrefs = {
  sound: { done: true, failed: true },
  notify: { done: true, failed: true, approval: true },
};

const booleans = (value: unknown, keys: string[]) =>
  typeof value === "object" && value !== null && keys.every((key) => typeof (value as Record<string, unknown>)[key] === "boolean");

export const isAlertPrefs = (value: unknown): value is AlertPrefs =>
  typeof value === "object" &&
  value !== null &&
  booleans((value as AlertPrefs).sound, ["done", "failed"]) &&
  booleans((value as AlertPrefs).notify, ["done", "failed", "approval"]);

/** The sound for a moment, if any: an end the agent reached, never a card. */
export function soundFor(attention: Attention, prefs: AlertPrefs): Sound | null {
  if (attention === "approval" || !prefs.sound[attention]) return null;
  return attention === "done" ? "done" : "attention";
}

// As `lib/window.ts` has it; not imported from there, which would bring the
// window API along for nothing.

/**
 * A system notification, only while the window is in the background — in
 * front, the chat already shows it. Permission is asked the first time one is
 * due; refused, nothing is shown and nothing is asked again by the system.
 */
export async function notifyInBackground(title: string, body: string) {
  // The page's own focus, which follows the window's: no IPC, and no
  // dependency on the window API for one boolean.
  if (!inTauri() || document.hasFocus()) return;
  let granted = await isPermissionGranted();
  if (!granted) granted = (await requestPermission()) === "granted";
  if (granted) sendNotification({ title, body });
}
