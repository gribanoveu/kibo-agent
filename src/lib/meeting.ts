import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./backend";

/** Meeting notes (`domain::meeting`, docs/36): a call's recording made into a
    transcript with speakers and times, and that into the model's notes. */
export type Utterance = {
  /** From 0, by who spoke first. */
  speaker: number;
  start: number;
  end: number;
  text: string;
};

export type Meeting = {
  id: string;
  /** The recording's file name. */
  name: string;
  /** RFC 3339. */
  created: string;
  seconds: number;
  utterances: Utterance[];
  summary?: string;
};

export type MeetingInfo = {
  id: string;
  name: string;
  created: string;
  seconds: number;
  speakers: number;
  summarized: boolean;
};

export type Stage = "decode" | "speech" | "speakers";

export type MeetingEvent =
  | { kind: "progress"; id: string; name: string; stage: Stage; done: number; total: number }
  /** The transcript is saved: read the meeting again. */
  | { kind: "transcribed"; id: string }
  | { kind: "summaryDelta"; id: string; text: string };

export const MEETING_EVENT = "meeting:event";

export async function onMeetingEvent(handler: (event: MeetingEvent) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<MeetingEvent>(MEETING_EVENT, (event) => handler(event.payload));
}

const requireBackend = () => {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
};

/** Newest first. */
export async function meetingList(): Promise<MeetingInfo[]> {
  if (!inTauri()) return [];
  return invoke<MeetingInfo[]>("meeting_list");
}

export async function meetingGet(id: string): Promise<Meeting> {
  requireBackend();
  return invoke<Meeting>("meeting_get", { id });
}

export async function meetingDelete(id: string): Promise<void> {
  requireBackend();
  return invoke("meeting_delete", { id });
}

/** The recording recognized, saved and summarized; its id once all is done. */
export async function meetingProcess(path: string): Promise<string> {
  requireBackend();
  return invoke<string>("meeting_process", { path });
}

/** The notes written again. */
export async function meetingSummarize(id: string): Promise<void> {
  requireBackend();
  return invoke("meeting_summarize", { id });
}

/** Stops what runs now, recognition or notes. */
export async function meetingStop(): Promise<void> {
  if (!inTauri()) return;
  return invoke("meeting_stop");
}
