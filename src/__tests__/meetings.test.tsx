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

const info = (m: Meeting) => ({
  id: m.id,
  name: m.name,
  title: m.title,
  created: m.created,
  seconds: m.seconds,
  speakers: new Set(m.utterances.map((u) => u.speaker)).size,
  summarized: m.summary !== undefined,
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
});

describe("the meetings page", () => {
  test("lists the meetings; one opens to its notes over its transcript", async () => {
    stored[0].utterances.push({ speaker: 1, start: 70, end: 72, text: "И напишу клиентам." });
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
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
    expect(screen.getByText("this recording cannot be read: no audio track")).toBeTruthy();
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
