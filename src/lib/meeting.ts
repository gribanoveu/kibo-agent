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
  /** What the notes found the meeting explained at length — worth a document of its own. */
  topics?: string[];
  /** The documents written from the meeting, one per subject. */
  documents?: MeetingDocument[];
};

/** Everything the meeting said on one subject, as the model wrote it up. */
export type MeetingDocument = { topic: string; text: string };

/** A voice the voice memory knows (`domain::voices`), and in how many meetings it was named. */
export type VoiceInfo = { id: string; name: string; meetings: number };

/** A microphone a call can be recorded from; `id` is what the settings keep. */
export type Microphone = { id: string; name: string };

/** The user's name for a speaker, or `Speaker N` from 1 — as the notes call whoever is not named. */
export const speakerName = (meeting: Meeting, speaker: number) => meeting.names?.[speaker] ?? `Speaker ${speaker + 1}`;

/** How many speakers there are: numbered from 0, the highest one plus one. */
export const speakerCount = (meeting: Meeting) => Math.max(0, ...meeting.utterances.map((u) => u.speaker + 1));

/** What a meeting is called: its title, or its file's name until the notes give it one. */
export const meetingTitle = (m: { name: string; title?: string }) => m.title ?? m.name;

/** Notes as they stream, without the `# title` line the model starts them with
    (`domain::meeting::split_title` takes it off the saved ones). */
export const withoutTitle = (notes: string) => notes.replace(/^\s*#(?!#)[^\n]*(\n|$)/, "").trimStart();

/** Notes as they stream, without the `## Documents` section that becomes their
    topics (`domain::meeting::split_topics` takes it off the saved ones). */
export const withoutTopics = (notes: string) => notes.replace(/^##[ \t]*documents[ \t]*$(\n(?!#).*)*/im, "").trimEnd();

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
  /** Its kept recording's size on disk; 0 when it has none. */
  bytes: number;
};

/** `notes`: a long meeting's parts read one by one, then the whole written. */
export type Stage = "decode" | "speech" | "speakers" | "notes";

export type MeetingEvent =
  | { kind: "progress"; id: string; name: string; stage: Stage; done: number; total: number }
  /** The transcript is saved: read the meeting again. */
  | { kind: "transcribed"; id: string }
  | { kind: "summaryDelta"; id: string; text: string }
  | { kind: "documentDelta"; id: string; topic: string; text: string }
  /** The job ended, however it ended: read the list and the meeting again. */
  | { kind: "ended" }
  /** A call being recorded: how long by now, how loud each side, 0 to 1 — at most ten a second. */
  | { kind: "recording"; id: string; seconds: number; mic: number; call: number }
  /** The call stopped being recorded without a Stop, and why; its file waits in the list. */
  | { kind: "recordingEnded"; id: string; error: string };

/** A call recorded and not yet made into a meeting (`domain::recording`). */
export type RecordingInfo = {
  id: string;
  /** When it began, RFC 3339. */
  created: string;
  seconds: number;
};

/** The call being recorded, as the page follows it. */
export type Recording = {
  id: string;
  seconds: number;
  /** The last moment's levels, 0 to 1. */
  mic: number;
  call: number;
  /** When the call's side was last heard, in the recording's seconds. */
  callHeardAt: number;
  /** Whether the microphone has heard anything yet. */
  micHeard: boolean;
  /** Your microphone written as silence for now. */
  muted: boolean;
};

/** A level past this is sound, not the noise of a quiet room. */
const HEARD = 0.01;
/** The call's side silent this long while the microphone hears is a missing permission, most likely. */
export const CALL_SILENCE_SECONDS = 120;

/** `now` as a level event about it leaves it; a recording the page did not know of starts here. */
export function hearLevels(now: Recording | null, event: { id: string; seconds: number; mic: number; call: number }): Recording {
  const was: Recording = now?.id === event.id ? now : { id: event.id, seconds: 0, mic: 0, call: 0, callHeardAt: event.seconds, micHeard: false, muted: false };
  return {
    id: event.id,
    seconds: event.seconds,
    mic: event.mic,
    call: event.call,
    callHeardAt: event.call > HEARD ? event.seconds : was.callHeardAt,
    micHeard: was.micHeard || event.mic > HEARD,
    muted: was.muted,
  };
}

/** Whether the call has been silent long enough, while you were heard, that its sound is likely not recorded. */
export const callSilent = (r: Recording) => r.micHeard && r.seconds - r.callHeardAt >= CALL_SILENCE_SECONDS;

/** Your microphone written as silence in the recording from now on, or heard again. */
export async function meetingRecordMute(on: boolean): Promise<void> {
  requireBackend();
  return invoke("meeting_record_mute", { on });
}

/** What runs now (`domain::meeting::JobState`): a recording being processed, notes written again, or a document. */
export type MeetingJob = {
  kind: "process" | "summary" | "document";
  /** The subject of the document being written. */
  topic?: string;
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

/** A document on `topic` written from meeting `id`, in place of the one it had. */
export async function meetingWriteDocument(id: string, topic: string): Promise<void> {
  requireBackend();
  return invoke("meeting_write_document", { id, topic });
}

/** The meeting after its document on `topic` is gone. */
export async function meetingDeleteDocument(id: string, topic: string): Promise<Meeting> {
  requireBackend();
  return invoke<Meeting>("meeting_delete_document", { id, topic });
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

/** The call being recorded now, if one is — for a page opened while it records. */
export async function meetingRecording(): Promise<{ id: string; seconds: number; muted: boolean } | null> {
  if (!inTauri()) return null;
  return invoke<{ id: string; seconds: number; muted: boolean } | null>("meeting_recording");
}

/** The calls recorded and not yet transcribed, newest first. */
export async function meetingRecordings(): Promise<RecordingInfo[]> {
  if (!inTauri()) return [];
  return invoke<RecordingInfo[]>("meeting_recordings");
}

/** The recorded call made into a meeting, as an import is; the meeting's id once all is done. */
export async function meetingProcessRecording(id: string): Promise<string> {
  requireBackend();
  return invoke<string>("meeting_process_recording", { id });
}

export async function meetingRecordingDelete(id: string): Promise<void> {
  requireBackend();
  return invoke("meeting_recording_delete", { id });
}

/** What a recording needs the system's leave for: your microphone, and the call's sound. */
export type Source = "microphone" | "call";
/** `denied`: no prompt asks again, only System Settings; `unknown`: the system does not say. */
export type Access = "granted" | "denied" | "notAsked" | "unknown";
export type RecordingAccess = Record<Source, Access>;

/** Whether the system lets the app record each side. */
export async function meetingRecordingAccess(): Promise<RecordingAccess> {
  if (!inTauri()) return { microphone: "unknown", call: "unknown" };
  return invoke<RecordingAccess>("meeting_recording_access");
}

/** The system's prompt for `source`, answered; the access after. */
export async function meetingRecordingAccessRequest(source: Source): Promise<Access> {
  requireBackend();
  return invoke<Access>("meeting_recording_access_request", { source });
}

/** System Settings opened where `source` is turned on. */
export async function meetingRecordingAccessSettings(source: Source): Promise<void> {
  requireBackend();
  return invoke("meeting_recording_access_settings", { source });
}

/** Whether a recorded call's microphone is denoised when it is transcribed — on unless turned off. */
export async function meetingDenoiseGet(): Promise<boolean> {
  if (!inTauri()) return true;
  return invoke<boolean>("meeting_denoise_get");
}

export async function meetingDenoiseSet(on: boolean): Promise<void> {
  requireBackend();
  return invoke("meeting_denoise_set", { on });
}

/** The microphones plugged in. */
export async function meetingMicrophones(): Promise<Microphone[]> {
  if (!inTauri()) return [];
  return invoke<Microphone[]>("meeting_microphones");
}

/** The microphone a call is recorded from; `null` is the system's default. */
export async function meetingMicrophoneGet(): Promise<string | null> {
  if (!inTauri()) return null;
  return invoke<string | null>("meeting_microphone_get");
}

export async function meetingMicrophoneSet(id: string | null): Promise<void> {
  requireBackend();
  return invoke("meeting_microphone_set", { id });
}

/** Whether a recording is transcribed as soon as it is stopped. */
export async function meetingAutoProcessGet(): Promise<boolean> {
  if (!inTauri()) return false;
  return invoke<boolean>("meeting_auto_process_get");
}

export async function meetingAutoProcessSet(on: boolean): Promise<void> {
  requireBackend();
  return invoke("meeting_auto_process_set", { on });
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
