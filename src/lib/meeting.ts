import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./backend";
import { windowsFrame } from "./window";

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
  /** A few words on what it was about, written with the notes. */
  title?: string;
  /** When it was recognized, RFC 3339. */
  created: string;
  seconds: number;
  utterances: Utterance[];
  summary?: string;
  /** What the user called a speaker, by number from 0. */
  names?: Record<string, string>;
  /** The speakers whose name the voice memory gave, not the user. */
  guessed?: number[];
};

/** A voice the voice memory knows (`domain::voices`), and in how many meetings it was named. */
export type VoiceInfo = { id: string; name: string; meetings: number };

/** The user's name for a speaker, or `Speaker N` from 1 — as the notes call whoever is not named. */
export const speakerName = (meeting: Meeting, speaker: number) => meeting.names?.[speaker] ?? `Speaker ${speaker + 1}`;

/** How many speakers there are: numbered from 0, the highest one plus one. */
export const speakerCount = (meeting: Meeting) => Math.max(0, ...meeting.utterances.map((u) => u.speaker + 1));

/** What a meeting is called: its title, or its file's name until the notes give it one. */
export const meetingTitle = (m: { name: string; title?: string }) => m.title ?? m.name;

/** Notes as they stream, without the `# title` line the model starts them with
    (`domain::meeting::split_title` takes it off the saved ones). */
export const withoutTitle = (notes: string) => notes.replace(/^\s*#(?!#)[^\n]*(\n|$)/, "").trimStart();

/** Where the page's player reads meeting `id`'s recording (`infra::meeting_audio`).
    Windows serves an app's scheme as `http://<scheme>.localhost`. */
export const audioUrl = (id: string, windows = windowsFrame) =>
  `${windows ? "http://meeting.localhost" : "meeting://localhost"}/${encodeURIComponent(id)}`;

export type MeetingInfo = {
  id: string;
  name: string;
  title?: string;
  created: string;
  seconds: number;
  speakers: number;
  summarized: boolean;
};

/** `notes`: a long meeting's parts read one by one, then the whole written. */
export type Stage = "decode" | "speech" | "speakers" | "notes";

export type MeetingEvent =
  | { kind: "progress"; id: string; name: string; stage: Stage; done: number; total: number }
  /** The transcript is saved: read the meeting again. */
  | { kind: "transcribed"; id: string }
  | { kind: "summaryDelta"; id: string; text: string }
  /** The job ended, however it ended: read the list and the meeting again. */
  | { kind: "ended" };

/** What runs now (`domain::meeting::JobState`): a recording being processed, or notes written again. */
export type MeetingJob = {
  kind: "process" | "summary";
  /** Known once the backend names it — at once for notes, on the first progress for a recording. */
  id?: string;
  name?: string;
  stage?: Stage;
  done?: number;
  total?: number;
};

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

/** The meeting after speaker `speaker` is called `name`; blank gives back its number. The notes follow. */
export async function meetingRenameSpeaker(id: string, speaker: number, name: string): Promise<Meeting> {
  requireBackend();
  return invoke<Meeting>("meeting_rename_speaker", { id, speaker, name });
}

/** The notes written again. */
export async function meetingSummarize(id: string): Promise<void> {
  requireBackend();
  return invoke("meeting_summarize", { id });
}

/** The job running now, if one is — for a page opened while it runs. */
export async function meetingJob(): Promise<MeetingJob | null> {
  if (!inTauri()) return null;
  return invoke<MeetingJob | null>("meeting_job");
}

/** Stops what runs now, recognition or notes. */
export async function meetingStop(): Promise<void> {
  if (!inTauri()) return;
  return invoke("meeting_stop");
}

/** The call recorded from now on — the microphone and what the computer plays; the recording's id. */
export async function meetingRecordStart(): Promise<string> {
  requireBackend();
  return invoke<string>("meeting_record_start");
}

/** The recording stopped and its file whole; its id. */
export async function meetingRecordStop(): Promise<string> {
  requireBackend();
  return invoke<string>("meeting_record_stop");
}

/** The voices the voice memory knows, by name. */
export async function meetingVoices(): Promise<VoiceInfo[]> {
  if (!inTauri()) return [];
  return invoke<VoiceInfo[]>("meeting_voices");
}

/** The voice forgotten with every sample of it; the list after. */
export async function meetingVoiceDelete(id: string): Promise<VoiceInfo[]> {
  requireBackend();
  return invoke<VoiceInfo[]>("meeting_voice_delete", { id });
}
