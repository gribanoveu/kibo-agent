import { Channel, invoke } from "@tauri-apps/api/core";
import { inTauri } from "./backend";
import type { Access } from "./meeting";
import { shortcutText } from "./shortcuts";

/** What the message box hears of a dictation (`domain::dictation::DictationEvent`). */
export type DictationEvent =
  | { kind: "preparing" }
  | { kind: "listening" }
  | { kind: "levels"; peaks: [number, number, number, number] }
  | { kind: "partial"; text: string }
  | { kind: "phrase"; text: string }
  | { kind: "ended"; reason: "stopped" | "cancelled" | "limit" }
  | { kind: "failed"; message: string };

/** Resolves once it is begun; a refusal rejects, and everything after comes to `onEvent`. */
export async function dictationStart(onEvent: (event: DictationEvent) => void): Promise<void> {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
  const channel = new Channel<DictationEvent>();
  channel.onmessage = onEvent;
  await invoke("dictation_start", { onEvent: channel });
}

/** What was said stays: the rest of it comes to `onEvent`, then its end. */
export async function dictationStop(): Promise<void> {
  if (inTauri()) await invoke("dictation_stop");
}

/** What was said is thrown away. */
export async function dictationCancel(): Promise<void> {
  if (inTauri()) await invoke("dictation_cancel");
}

/** How long the key is held before it is a dictation and not ⌥ on its way to another key. */
export const HOLD_MS = 200;
/** How many peaks the wave shows: a little over a second of them. */
export const WAVE_BARS = 48;

/** `finishing`: Stop pressed, the end of what was said still being recognized. */
export type DictationPhase = "idle" | "preparing" | "listening" | "finishing";

export type DictationView = {
  phase: DictationPhase;
  /** The phrase being said, as far as it is recognized. */
  partial: string;
  /** The wave, oldest first. */
  peaks: number[];
  error: string | null;
};

export const NOT_DICTATING: DictationView = { phase: "idle", partial: "", peaks: [], error: null };

/** What the channel says, and what is known here first: a Stop pressed, a start refused, something else gone wrong, the note closed. */
export type DictationAction = DictationEvent | { kind: "stopping" } | { kind: "refused"; message: string } | { kind: "noted"; message: string } | { kind: "dismissed" };

export function dictationStep(view: DictationView, action: DictationAction): DictationView {
  switch (action.kind) {
    case "preparing":
      return { ...NOT_DICTATING, phase: "preparing" };
    case "listening":
      // A Stop can get ahead of the first buffer.
      return view.phase === "preparing" ? { ...view, phase: "listening" } : view;
    case "levels":
      return { ...view, peaks: [...view.peaks, ...action.peaks].slice(-WAVE_BARS) };
    case "partial":
      return { ...view, partial: action.text };
    case "phrase":
      return { ...view, partial: "" };
    case "stopping":
      return view.phase === "idle" ? view : { ...view, phase: "finishing" };
    case "ended":
      return NOT_DICTATING;
    case "failed":
    case "refused":
      return { ...NOT_DICTATING, error: action.message };
    case "noted":
      return { ...view, error: action.message };
    case "dismissed":
      return { ...view, error: null };
  }
}

/** What goes before a phrase put at `at`: a space, unless the line starts there or one is there. */
const leadIn = (text: string, at: number) => (at === 0 || /\s/.test(text[at - 1]) ? "" : " ");

/** `phrase` put in `text` at `at`, and where the next one goes. */
export function insertPhrase(text: string, at: number, phrase: string): { text: string; at: number } {
  const place = Math.min(at, text.length);
  const said = leadIn(text, place) + phrase;
  const rest = text.slice(place);
  const after = /^[\p{L}\p{N}]/u.test(rest) ? " " : "";
  return { text: text.slice(0, place) + said + after + rest, at: place + said.length };
}

/** What the microphone button is now, and what pressing it does; `null`: nothing. */
export type MicButton = { look: "mic" | "refused" | "working" | "stop"; title: string; press: "start" | "settings" | "cancel" | "stop" | null };

export function micButton(access: Access | undefined, phase: DictationPhase): MicButton {
  if (phase === "preparing") return { look: "working", title: "Opening the microphone — click to cancel", press: "cancel" };
  if (phase === "listening") return { look: "stop", title: "Stop — what you said stays", press: "stop" };
  if (phase === "finishing") return { look: "working", title: "Recognizing the end of it", press: null };
  if (access === "denied") return { look: "refused", title: "The app may not use the microphone — click to open the system's settings", press: "settings" };
  return { look: "mic", title: `Dictate — or hold ${shortcutText("dictate")}`, press: "start" };
}

/**
 * The key held to dictate. `press` starts it once held for [`HOLD_MS`];
 * `other`, another key meanwhile, means it was a modifier and not a dictation;
 * `lift` is the key let go, or the window losing the focus its `keyup` would come to.
 */
export function holdKey(start: () => void, release: () => void) {
  let waiting: ReturnType<typeof setTimeout> | null = null;
  let held = false;
  const abandon = () => {
    if (waiting !== null) clearTimeout(waiting);
    waiting = null;
  };
  return {
    press() {
      if (held || waiting !== null) return;
      waiting = setTimeout(() => {
        waiting = null;
        held = true;
        start();
      }, HOLD_MS);
    },
    other: abandon,
    lift() {
      abandon();
      if (held) release();
      held = false;
    },
  };
}
