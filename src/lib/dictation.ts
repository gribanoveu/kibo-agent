import { Channel, invoke } from "@tauri-apps/api/core";

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
  const channel = new Channel<DictationEvent>();
  channel.onmessage = onEvent;
  await invoke("dictation_start", { onEvent: channel });
}

/** What was said stays: the rest of it comes to `onEvent`, then its end. */
export async function dictationStop(): Promise<void> {
  await invoke("dictation_stop");
}

/** What was said is thrown away. */
export async function dictationCancel(): Promise<void> {
  await invoke("dictation_cancel");
}
