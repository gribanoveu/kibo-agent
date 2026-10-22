import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Meeting, MeetingEvent } from "../lib/meeting";

// The Meetings page (docs/36): a recording in, its notes and transcript out.
// The backend's job reports on `meeting:event`, each event naming its meeting;
// the page follows the one it started and ignores the rest.

let stored: Meeting[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let finishProcess: (result: { id?: string; error?: string }) => void = () => {};
let voiceMemoryOn = false;
let voices: { id: string; name: string; meetings: number }[] = [];
let running: unknown = null;
let recordings: { id: string; created: string; seconds: number }[] = [];
let recordingNow: { id: string; seconds: number; muted: boolean } | null = null;
let autoOn = false;
let denoiseOn = true;
let finishRecording: (result: { id?: string; error?: string }) => void = () => {};
let access: Record<string, string> = { microphone: "granted", call: "granted" };

const info = (m: Meeting) => ({
  id: m.id,
  name: m.name,
  title: m.title,
  created: m.created,
  seconds: m.seconds,
  speakers: new Set(m.utterances.map((u) => u.speaker)).size,
  summarized: m.summary !== undefined,
  bytes: m.id === "m1" ? 24_200_000 : 0,
});

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "features_list") return Promise.resolve(voiceMemoryOn ? [{ id: "voice-memory", on: true }] : []);
    if (command === "meeting_voices") return Promise.resolve(structuredClone(voices));
    if (command === "meeting_voice_delete") {
      voices = voices.filter((v) => v.id !== args!.id);
      return Promise.resolve(structuredClone(voices));
    }
    if (command === "meeting_job") return Promise.resolve(running);
    if (command === "meeting_recording") return Promise.resolve(recordingNow);
    if (command === "meeting_recording_access") return Promise.resolve({ ...access });
    if (command === "meeting_recording_access_request") {
      access[String(args!.source)] = "granted";
      return Promise.resolve("granted");
    }
    if (command === "meeting_recordings") return Promise.resolve(structuredClone(recordings));
    if (command === "meeting_auto_process_get") return Promise.resolve(autoOn);
    if (command === "meeting_denoise_get") return Promise.resolve(denoiseOn);
    if (command === "meeting_denoise_set") {
      denoiseOn = Boolean(args!.on);
      return Promise.resolve(null);
    }
    if (command === "meeting_auto_process_set") {
      autoOn = Boolean(args!.on);
      return Promise.resolve(null);
    }
    if (command === "meeting_record_mute") {
      if (!recordingNow) return Promise.reject("nothing is being recorded");
      recordingNow.muted = Boolean(args!.on);
      return Promise.resolve(null);
    }
    if (command === "meeting_record_start") {
      recordingNow = { id: "r1", seconds: 0, muted: false };
      return Promise.resolve("r1");
    }
    if (command === "meeting_record_stop") {
      const id = recordingNow?.id;
      recordingNow = null;
      if (!id) return Promise.reject("nothing is being recorded");
      recordings = [{ id, created: "2026-10-21T10:00:00Z", seconds: 95 }, ...recordings];
      return Promise.resolve(id);
    }
    if (command === "meeting_recording_delete") {
      recordings = recordings.filter((r) => r.id !== args!.id);
      return Promise.resolve(null);
    }
    if (command === "meeting_process_recording") {
      return new Promise((resolve, reject) => {
        finishRecording = ({ id, error }) => {
          if (!error) recordings = recordings.filter((r) => r.id !== args!.id);
          return error ? reject(error) : resolve(id);
        };
      });
    }
    if (command === "meeting_list") return Promise.resolve(stored.map(info));
    if (command === "meeting_get") return Promise.resolve(structuredClone(stored.find((m) => m.id === args!.id)));
    if (command === "meeting_delete") {
      stored = stored.filter((m) => m.id !== args!.id);
      return Promise.resolve(null);
    }
    if (command === "meeting_rename_speaker") {
      const m = stored.find((x) => x.id === args!.id)!;
      const name = String(args!.name).trim();
      m.names = { ...m.names };
      if (name) m.names[String(args!.speaker)] = name;
      else delete m.names[String(args!.speaker)];
      return Promise.resolve(structuredClone(m));
    }
    if (command === "meeting_process") {
      return new Promise((resolve, reject) => {
        finishProcess = ({ id, error }) => (error ? reject(error) : resolve(id));
      });
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Set<(event: { payload: MeetingEvent }) => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (event: { payload: MeetingEvent }) => void) => {
    if (channel === "meeting:event") listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));

mock.module("@tauri-apps/plugin-dialog", () => ({
  open: () => Promise.resolve("/calls/standup.m4a"),
  save: () => Promise.resolve(null),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { MeetingsPage, clock, day } = await import("../components/MeetingsPage");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const emit = (payload: MeetingEvent) => act(() => listeners.forEach((l) => l({ payload })));

const call: Meeting = {
  id: "m1",
  name: "planning.wav",
  created: "2026-10-19T09:00:00Z",
  seconds: 754,
  utterances: [
    { speaker: 0, start: 3, end: 5, text: "Выпускаем в пятницу." },
    { speaker: 1, start: 65, end: 67, text: "Я подготовлю релиз." },
  ],
  summary: "## Decisions\n- Release on Friday.",
};

beforeEach(() => {
  calls = [];
  stored = [structuredClone(call)];
  voiceMemoryOn = false;
  voices = [];
  running = null;
  recordings = [];
  recordingNow = null;
  autoOn = false;
  denoiseOn = true;
  access = { microphone: "granted", call: "granted" };
});

describe("the meetings page", () => {
  test("lists the meetings; one opens to its notes over its transcript", async () => {
    stored[0].utterances.push({ speaker: 1, start: 70, end: 72, text: "И напишу клиентам." });
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    expect(screen.getByTitle("Size of the recording").textContent).toBe("23.1 MB");
    const notes = within(screen.getByRole("region", { name: "Notes" }));
    expect(notes.getByText("Release on Friday.")).toBeTruthy();
    const transcript = within(screen.getByRole("region", { name: "Transcript" }));
    expect(screen.queryByText("Import recording")).toBeNull();
    expect(transcript.getAllByText("Speaker 2").length).toBe(2); // its chip and its one turn of two lines
    expect(transcript.getByText("И напишу клиентам.")).toBeTruthy();
    expect(transcript.getByText("1:05")).toBeTruthy();
    expect(transcript.getByText("Я подготовлю релиз.")).toBeTruthy();

    fireEvent.click(screen.getByText("All meetings"));
    await settle();
    expect(screen.getByTitle("Speakers").textContent).toBe("2");
    expect(screen.getByTitle("Duration").textContent).toBe("12:34");
    expect(screen.queryByTitle("Size of the recording")).toBeNull();
  });

  test("a meeting whose recording is gone shows no size", async () => {
    stored = [{ ...structuredClone(call), id: "m9" }];
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    expect(screen.getByTitle("Duration").textContent).toBe("12:34");
    expect(screen.queryByTitle("Size of the recording")).toBeNull();
  });

  test("a day's meetings are under its heading", () => {
    const now = new Date(2026, 9, 20, 18, 0);
    expect(day(new Date(2026, 9, 20, 0, 5).toISOString(), now)).toBe("Today");
    expect(day(new Date(2026, 9, 19, 23, 55).toISOString(), now)).toBe("Yesterday");
    expect(day(new Date(2026, 9, 17, 12).toISOString(), now)).not.toMatch(/2026/);
    expect(day(new Date(2025, 9, 17, 12).toISOString(), now)).toMatch(/2025/);
  });

  test("a turn plays the recording from where it starts; a meeting without one says so", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    const audio = document.querySelector("audio")!;
    expect(audio.getAttribute("src")).toBe("meeting://localhost/m1");
    let played = 0;
    audio.play = () => {
      played++;
      return Promise.resolve();
    };
    fireEvent.click(screen.getByLabelText("Play from 1:05"));
    expect(audio.currentTime).toBe(65);
    expect(played).toBe(1);
    expect((screen.getByLabelText("Position") as HTMLInputElement).value).toBe("65");

    fireEvent.error(audio);
    expect(screen.getByText("No recording kept for this meeting")).toBeTruthy();
    expect((screen.getByLabelText("Play from 1:05") as HTMLButtonElement).disabled).toBe(true);
  });

  test("each meeting says whether it has notes; the job's stage and share are on its card", async () => {
    stored.push({ ...structuredClone(call), id: "m0", name: "raw.wav", created: "2026-10-18T09:00:00Z", summary: undefined });
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.getByText("Notes")).toBeTruthy();
    expect(screen.getByText("Transcript only")).toBeTruthy();

    fireEvent.click(screen.getByText("Import recording"));
    await settle();
    await emit({ kind: "progress", id: "m2", name: "standup.m4a", stage: "speakers", done: 30, total: 60 });
    const card = screen.getByRole("status");
    expect(card.textContent).toContain("standup.m4a");
    expect(card.textContent).toContain("Step 3 of 4");
    expect(card.textContent).toContain("Telling the voices apart… 50%");
    await emit({ kind: "progress", id: "m2", name: "standup.m4a", stage: "speakers", done: 60, total: 60 });
    expect(screen.getByRole("status").textContent).toContain("Grouping the voices…");
    expect(screen.getByRole("status").textContent).not.toContain("%");
  });

  test("a meeting is listed by the title its notes gave it", async () => {
    stored[0].title = "Выпуск в пятницу";
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.queryByText("planning.wav")).toBeNull();
    fireEvent.click(screen.getByText("Выпуск в пятницу"));
    await settle();
    expect(screen.getByRole("heading", { name: "Выпуск в пятницу" })).toBeTruthy();
  });

  test("a recording added shows its stage, opens on its transcript, and streams only its own notes", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Import recording"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "meeting_process", args: { path: "/calls/standup.m4a" } });
    expect((screen.getByText("Import recording").closest("button") as HTMLButtonElement).disabled).toBe(true);

    await emit({ kind: "progress", id: "m2", name: "standup.m4a", stage: "speech", done: 1, total: 4 });
    expect(screen.getByRole("status").textContent).toContain("Recognizing speech… 25%");

    stored.unshift({ id: "m2", name: "standup.m4a", created: "2026-10-20T09:00:00Z", seconds: 30, utterances: [{ speaker: 0, start: 0, end: 1, text: "Привет." }] });
    await emit({ kind: "transcribed", id: "m2" });
    await settle();
    expect(screen.getByRole("article", { name: "standup.m4a" })).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Writing the notes");

    await emit({ kind: "summaryDelta", id: "m1", text: "someone else's" });
    await emit({ kind: "summaryDelta", id: "m2", text: "# Greet" });
    expect(screen.getByRole("region", { name: "Notes" }).textContent).not.toContain("Greet");
    await emit({ kind: "summaryDelta", id: "m2", text: "ings\n\nSaid hello." });
    // Streamed text is split into word spans: read the region's text.
    const notes = screen.getByRole("region", { name: "Notes" }).textContent;
    expect(notes).not.toContain("someone else's");
    expect(notes).toContain("Said hello.");

    stored[0].summary = "Said hello.";
    stored[0].title = "Greetings";
    await act(async () => finishProcess({ id: "m2" }));
    await settle();
    expect(screen.getByRole("heading", { name: "Greetings" })).toBeTruthy();
    expect(screen.getByTitle("Imported from").textContent).toBe("standup.m4a");
    expect(screen.queryByRole("status")).toBeNull();
    expect(within(screen.getByRole("region", { name: "Notes" })).getByText("Write again")).toBeTruthy();
  });

  test("a stop is not reported as a failure, a real failure is", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Import recording"));
    await settle();
    fireEvent.click(screen.getByText("Stop"));
    expect(calls.at(-1)?.command).toBe("meeting_stop");
    await act(async () => finishProcess({ error: "stopped" }));
    await settle();
    expect(screen.queryByText("stopped")).toBeNull();

    fireEvent.click(screen.getByText("Import recording"));
    await settle();
    await act(async () => finishProcess({ error: "this recording cannot be read: no audio track" }));
    await settle();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("this recording cannot be read: no audio track");
    fireEvent.click(within(alert).getByLabelText("Close"));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  test("a page opened while a job runs shows it, can stop it, and lets go when it ends", async () => {
    running = { kind: "process", id: "m2", name: "standup.m4a", stage: "speech", done: 1, total: 4 };
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.getByRole("status").textContent).toContain("Recognizing speech… 25%");
    expect((screen.getByText("Import recording").closest("button") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByText("Stop"));
    expect(calls.at(-1)?.command).toBe("meeting_stop");

    running = null;
    await emit({ kind: "ended" });
    await settle();
    expect(screen.queryByRole("status")).toBeNull();
    expect((screen.getByText("Import recording").closest("button") as HTMLButtonElement).disabled).toBe(false);
  });

  test("a call turned away as busy leaves the job that runs on its card", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Import recording"));
    await settle();
    // Another job held the slot all along.
    running = { kind: "summary", id: "m1", stage: "notes", done: 0, total: 0 };
    await act(async () => finishProcess({ error: "a recording is already being processed — wait for it or stop it" }));
    await settle();
    expect(screen.getByText("a recording is already being processed — wait for it or stop it")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Writing the notes");
  });

  test("a call recorded shows its length and both sides' levels, and once stopped waits in the list", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Record"));
    await settle();
    expect(calls.some((c) => c.command === "meeting_record_start")).toBe(true);
    const card = () => screen.getByRole("status", { name: "Recording" });
    expect((screen.getByText("Record").closest("button") as HTMLButtonElement).disabled).toBe(true);

    await emit({ kind: "recording", id: "r1", seconds: 65, mic: 0.25, call: 0.04 });
    expect(card().textContent).toContain("1:05");
    expect(within(card()).getByRole("meter", { name: "You" }).getAttribute("aria-valuenow")).toBe("0.25");
    expect(within(card()).getByRole("meter", { name: "Call" }).getAttribute("aria-valuenow")).toBe("0.04");

    fireEvent.click(within(card()).getByText("Stop"));
    await settle();
    // The recorder's last moment, told after the stop: late, not a new recording.
    await emit({ kind: "recording", id: "r1", seconds: 95, mic: 0.1, call: 0.1 });
    expect(screen.queryByRole("status", { name: "Recording" })).toBeNull();
    expect(screen.getByText("Not transcribed")).toBeTruthy();
    expect(screen.getByText("Recorded call")).toBeTruthy();
    expect(screen.getByText("1:35")).toBeTruthy();
    expect(calls.some((c) => c.command === "meeting_process_recording")).toBe(false);
  });

  test("a second click on Record or Stop while the first is on its way is not a second call", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Record"));
    fireEvent.click(screen.getByText("Record"));
    await settle();
    expect(calls.filter((c) => c.command === "meeting_record_start").length).toBe(1);

    const stop = within(screen.getByRole("status", { name: "Recording" })).getByText("Stop");
    fireEvent.click(stop);
    fireEvent.click(stop);
    await settle();
    expect(calls.filter((c) => c.command === "meeting_record_stop").length).toBe(1);
    expect(screen.queryByText("nothing is being recorded")).toBeNull();
    expect((screen.getByText("Record").closest("button") as HTMLButtonElement).disabled).toBe(false);
  });

  test("a recording that ends by itself says why and waits in the list", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Record"));
    await settle();
    expect(screen.getByRole("status", { name: "Recording" })).toBeTruthy();

    recordingNow = null;
    recordings = [{ id: "r1", created: "2026-10-21T10:00:00Z", seconds: 40 }];
    await emit({ kind: "recordingEnded", id: "r1", error: "meeting notes: No space left on device" });
    await settle();
    expect(screen.queryByRole("status", { name: "Recording" })).toBeNull();
    expect(screen.getByText("meeting notes: No space left on device")).toBeTruthy();
    expect(screen.getByText("Recorded call")).toBeTruthy();
    // Its last level, told late, does not bring it back.
    await emit({ kind: "recording", id: "r1", seconds: 40, mic: 0.1, call: 0.1 });
    expect(screen.queryByRole("status", { name: "Recording" })).toBeNull();
  });

  test("your microphone is muted and heard again while the call records; a page opened later knows", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Record"));
    await settle();
    const card = () => screen.getByRole("status", { name: "Recording" });
    expect(card().textContent).not.toContain("Muted");
    fireEvent.click(within(card()).getByText("Mute"));
    await settle();
    expect(calls.some((c) => c.command === "meeting_record_mute" && c.args?.on === true)).toBe(true);
    expect(card().textContent).toContain("Muted");
    // A level event keeps it so.
    await emit({ kind: "recording", id: "r1", seconds: 3, mic: 0, call: 0.2 });
    expect(within(card()).getByText("Unmute").closest("button")!.getAttribute("aria-pressed")).toBe("true");

    cleanup();
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(card().textContent).toContain("Muted");
    fireEvent.click(within(card()).getByText("Unmute"));
    await settle();
    expect(recordingNow!.muted).toBe(false);
    expect(card().textContent).not.toContain("Muted");
  });

  test("Transcribe makes a recording a meeting; one asked for while a job runs waits for it", async () => {
    recordings = [
      { id: "r1", created: "2026-10-21T10:00:00Z", seconds: 60 },
      { id: "r2", created: "2026-10-21T09:00:00Z", seconds: 30 },
    ];
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    const [first, second] = screen.getAllByText("Transcribe");
    fireEvent.click(first);
    await settle();
    expect(calls.filter((c) => c.command === "meeting_process_recording").map((c) => c.args?.id)).toEqual(["r1"]);
    expect(screen.getByRole("status").textContent).toContain("Starting");
    // The one being transcribed is neither asked for again nor deleted under the job.
    expect(screen.getByText("Transcribing")).toBeTruthy();
    expect(screen.getAllByText("Transcribe")).toHaveLength(1);
    expect((screen.getAllByLabelText(/^Delete Recorded call/)[0] as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(second);
    await settle();
    expect(screen.getByText("Waiting")).toBeTruthy();
    const waiting = screen.getAllByLabelText(/^Delete Recorded call/)[1] as HTMLButtonElement;
    expect(waiting.disabled).toBe(true); // it is about to be read
    expect(calls.filter((c) => c.command === "meeting_process_recording")).toHaveLength(1);

    await act(async () => finishRecording({ id: "m9" }));
    await settle();
    expect(calls.filter((c) => c.command === "meeting_process_recording").map((c) => c.args?.id)).toEqual(["r1", "r2"]);
    await act(async () => finishRecording({ id: "m10" }));
    await settle();
    expect(screen.queryByText("Not transcribed")).toBeNull();
  });

  test("with Auto-transcribe on, a recording stopped is transcribed at once", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.queryByRole("switch")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Recording settings" }));
    const toggle = screen.getByRole("switch", { name: "Transcribe when a recording stops" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    await settle();
    expect(autoOn).toBe(true);
    expect(toggle.getAttribute("aria-checked")).toBe("true");

    fireEvent.click(screen.getByText("Record"));
    await settle();
    fireEvent.click(within(screen.getByRole("status", { name: "Recording" })).getByText("Stop"));
    await settle();
    expect(calls.filter((c) => c.command === "meeting_process_recording").map((c) => c.args?.id)).toEqual(["r1"]);
  });

  test("noise reduction on the microphone is on until turned off, and stays as set", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    const toggle = () => screen.getByRole("switch", { name: "Reduce noise on my side before transcribing" });
    const gear = () => screen.getByRole("button", { name: "Recording settings" });
    fireEvent.click(gear());
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    fireEvent.click(toggle());
    await settle();
    expect(denoiseOn).toBe(false);
    expect(toggle().getAttribute("aria-checked")).toBe("false");

    // A click inside keeps it open; Escape and a click outside close it.
    expect(gear().getAttribute("aria-expanded")).toBe("true");
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(screen.queryByRole("switch")).toBeNull();
    fireEvent.click(gear());
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("switch")).toBeNull();

    cleanup();
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(gear());
    expect(toggle().getAttribute("aria-checked")).toBe("false");
  });

  test("a page opened while a call is recorded shows it; a silent call says to check the permission", async () => {
    recordingNow = { id: "r7", seconds: 30, muted: false };
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    const card = () => screen.getByRole("status", { name: "Recording" });
    expect(card().textContent).toContain("0:30");
    await emit({ kind: "recording", id: "r7", seconds: 200, mic: 0, call: 0 });
    expect(card().textContent).not.toContain("No sound from the call"); // nobody spoke at all
    await emit({ kind: "recording", id: "r7", seconds: 201, mic: 0, call: 0.2 });
    await emit({ kind: "recording", id: "r7", seconds: 300, mic: 0.3, call: 0 });
    expect(card().textContent).not.toContain("No sound from the call"); // not two minutes since the call was last heard
    await emit({ kind: "recording", id: "r7", seconds: 330, mic: 0.3, call: 0 });
    expect(card().textContent).toContain("No sound from the call");
    await emit({ kind: "recording", id: "r7", seconds: 331, mic: 0.3, call: 0.2 });
    expect(card().textContent).not.toContain("No sound from the call");
  });

  test("each side the system has not allowed has its own note, with its way to allow it", async () => {
    access = { microphone: "notAsked", call: "denied" };
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    const mic = () => screen.queryByRole("note", { name: "Microphone permission" });
    const call = () => screen.queryByRole("note", { name: "System audio permission" });
    const record = () => screen.getByText("Record").closest("button") as HTMLButtonElement;
    expect(record().disabled).toBe(true); // a recording would ask past the notes
    expect(mic()!.textContent).toContain("macOS asks once");
    expect(call()!.textContent).toContain("Turned off in System Settings");

    fireEvent.click(within(mic()!).getByText("Allow"));
    await settle();
    expect(calls.some((c) => c.command === "meeting_recording_access_request" && c.args?.source === "microphone")).toBe(true);
    expect(mic()).toBeNull();
    expect(call()).not.toBeNull();
    expect(record().disabled).toBe(true); // one is still missing

    fireEvent.click(within(call()!).getByText("Open System Settings"));
    await settle();
    expect(calls.some((c) => c.command === "meeting_recording_access_settings" && c.args?.source === "call")).toBe(true);
    expect(calls.some((c) => c.command === "meeting_recording_access_request" && c.args?.source === "call")).toBe(false);

    // Turned on in System Settings, then back to the app.
    access.call = "granted";
    await act(async () => void window.dispatchEvent(new Event("focus")));
    await settle();
    expect(call()).toBeNull();
    expect(record().disabled).toBe(false);
  });

  test("nothing is said where the system does not say", async () => {
    access = { microphone: "unknown", call: "unknown" };
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.queryAllByRole("note")).toHaveLength(0);
    expect((screen.getByText("Record").closest("button") as HTMLButtonElement).disabled).toBe(false);
  });

  test("a recording is deleted only after it is confirmed", async () => {
    recordings = [{ id: "r1", created: "2026-10-21T10:00:00Z", seconds: 60 }];
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByLabelText(/^Delete Recorded call/));
    expect(screen.getByText("The recording goes, and with it the only copy of this call.")).toBeTruthy();
    fireEvent.click(screen.getByText("Delete"));
    await settle();
    expect(calls.some((c) => c.command === "meeting_recording_delete" && c.args?.id === "r1")).toBe(true);
    expect(screen.queryByText("Recorded call")).toBeNull();
  });

  test("a meeting is deleted only after it is confirmed", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByLabelText("Delete planning.wav"));
    expect(calls.some((c) => c.command === "meeting_delete")).toBe(false);
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Delete"));
    await settle();
    expect(calls.some((c) => c.command === "meeting_delete" && c.args?.id === "m1")).toBe(true);
    expect(screen.getByText(/No meetings yet/)).toBeTruthy();
  });

  test("a speaker renamed in place is called so in the transcript; Escape keeps the name", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    const speakers = within(screen.getByLabelText("Speakers"));
    fireEvent.click(speakers.getByText("Speaker 2"));
    const field = screen.getByLabelText("Name of speaker 2");
    fireEvent.change(field, { target: { value: "Илья" } });
    fireEvent.keyDown(field, { key: "Enter", code: "Enter" });
    await settle();
    expect(calls.at(-1)).toEqual({ command: "meeting_rename_speaker", args: { id: "m1", speaker: 1, name: "Илья" } });
    const transcript = within(screen.getByRole("region", { name: "Transcript" }));
    expect(transcript.getAllByText("Илья").length).toBe(2);
    expect(transcript.queryByText("Speaker 2")).toBeNull();

    const before = calls.length;
    fireEvent.click(speakers.getByText("Speaker 1"));
    fireEvent.change(screen.getByLabelText("Name of speaker 1"), { target: { value: "x" } });
    fireEvent.keyDown(screen.getByLabelText("Name of speaker 1"), { key: "Escape", code: "Escape" });
    await settle();
    expect(calls.length).toBe(before);
    expect(speakers.getByText("Speaker 1")).toBeTruthy();
  });

  test("a name typed is saved by clicking away or by ✓, unchanged it is not; × clears it", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    const speakers = within(screen.getByLabelText("Speakers"));
    const renames = () => calls.filter((c) => c.command === "meeting_rename_speaker");

    fireEvent.click(speakers.getByText("Speaker 1"));
    fireEvent.blur(screen.getByLabelText("Name of speaker 1"));
    await settle();
    expect(renames()).toEqual([]);

    fireEvent.click(speakers.getByText("Speaker 1"));
    fireEvent.change(screen.getByLabelText("Name of speaker 1"), { target: { value: "Оксана" } });
    fireEvent.blur(screen.getByLabelText("Name of speaker 1"));
    await settle();
    expect(renames().at(-1)?.args).toEqual({ id: "m1", speaker: 0, name: "Оксана" });

    fireEvent.click(speakers.getByText("Оксана"));
    fireEvent.click(screen.getByLabelText("Clear"));
    expect((screen.getByLabelText("Name of speaker 1") as HTMLInputElement).value).toBe("");
    fireEvent.click(screen.getByLabelText("Save name"));
    await settle();
    expect(renames().at(-1)?.args).toEqual({ id: "m1", speaker: 0, name: "" });
    expect(renames().length).toBe(2);
    expect(speakers.getByText("Speaker 1")).toBeTruthy();
  });

  test("a long meeting's notes say which part is being read", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Import recording"));
    await settle();
    await emit({ kind: "progress", id: "m3", name: "long.m4a", stage: "notes", done: 1, total: 4 });
    expect(screen.getByRole("status").textContent).toContain("Writing the notes… part 2 of 4");
    await emit({ kind: "progress", id: "m3", name: "long.m4a", stage: "notes", done: 4, total: 4 });
    expect(screen.getByRole("status").textContent).not.toContain("part");
    await act(async () => finishProcess({ id: "m3" }));
  });

  test("voices are listed only while the voice memory is on, and one is forgotten once confirmed", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    expect(screen.queryByText("Voices")).toBeNull();
    cleanup();

    voiceMemoryOn = true;
    voices = [{ id: "v1", name: "Альбина", meetings: 2 }];
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Voices"));
    await settle();
    const panel = within(screen.getByRole("complementary", { name: "Voices" }));
    expect(panel.getByText("Альбина")).toBeTruthy();
    expect(panel.getByText("2 meetings")).toBeTruthy();
    fireEvent.click(panel.getByLabelText("Forget Альбина"));
    expect(calls.some((c) => c.command === "meeting_voice_delete")).toBe(false);
    fireEvent.click(within(screen.getByRole("dialog")).getByText("Forget"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "meeting_voice_delete", args: { id: "v1" } });
    expect(panel.getByText(/No voices yet/)).toBeTruthy();
  });

  test("a name the voice memory gave says so on its speaker", async () => {
    stored[0].names = { "1": "Илья" };
    stored[0].guessed = [1];
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    const speakers = within(screen.getByLabelText("Speakers"));
    expect(speakers.getByText("Илья").getAttribute("title")).toContain("Recognized by voice");
    expect(speakers.getByText("Speaker 1").getAttribute("title")).toBe("Rename");

    // Kept as it is with ✓: the guess becomes the user's.
    fireEvent.click(speakers.getByText("Илья"));
    fireEvent.click(screen.getByLabelText("Save name"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "meeting_rename_speaker", args: { id: "m1", speaker: 1, name: "Илья" } });
  });

  test("times read as minutes, and hours past an hour", () => {
    expect([clock(5.4), clock(754), clock(3725)]).toEqual(["0:05", "12:34", "1:02:05"]);
  });
});
