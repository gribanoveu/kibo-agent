import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { Meeting, MeetingEvent } from "../lib/meeting";

// The Meetings page (docs/36): a recording in, its notes and transcript out.
// The backend's job reports on `meeting:event`, each event naming its meeting;
// the page follows the one it started and ignores the rest.

let stored: Meeting[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let finishProcess: (result: { id?: string; error?: string }) => void = () => {};

const info = (m: Meeting) => ({
  id: m.id,
  name: m.name,
  created: m.created,
  seconds: m.seconds,
  speakers: new Set(m.utterances.map((u) => u.speaker)).size,
  summarized: m.summary !== undefined,
});

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "meeting_list") return Promise.resolve(stored.map(info));
    if (command === "meeting_get") return Promise.resolve(structuredClone(stored.find((m) => m.id === args!.id)));
    if (command === "meeting_delete") {
      stored = stored.filter((m) => m.id !== args!.id);
      return Promise.resolve(null);
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

const { MeetingsPage, clock } = await import("../components/MeetingsPage");
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
});

describe("the meetings page", () => {
  test("lists the meetings; one opens to its notes over its transcript", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("planning.wav"));
    await settle();
    const notes = within(screen.getByRole("region", { name: "Notes" }));
    expect(notes.getByText("Release on Friday.")).toBeTruthy();
    const transcript = within(screen.getByRole("region", { name: "Transcript" }));
    expect(transcript.getByText("Speaker 2")).toBeTruthy();
    expect(transcript.getByText("1:05")).toBeTruthy();
    expect(transcript.getByText("Я подготовлю релиз.")).toBeTruthy();

    fireEvent.click(screen.getByText("All meetings"));
    await settle();
    expect(screen.getByText(/2 speakers/)).toBeTruthy();
  });

  test("a recording added shows its stage, opens on its transcript, and streams only its own notes", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Add recording"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "meeting_process", args: { path: "/calls/standup.m4a" } });
    expect((screen.getByText("Add recording").closest("button") as HTMLButtonElement).disabled).toBe(true);

    await emit({ kind: "progress", id: "m2", name: "standup.m4a", stage: "speech", done: 1, total: 4 });
    expect(screen.getByRole("status").textContent).toContain("Recognizing speech… 25%");

    stored.unshift({ id: "m2", name: "standup.m4a", created: "2026-10-20T09:00:00Z", seconds: 30, utterances: [{ speaker: 0, start: 0, end: 1, text: "Привет." }] });
    await emit({ kind: "transcribed", id: "m2" });
    await settle();
    expect(screen.getByRole("article", { name: "standup.m4a" })).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Writing the notes");

    await emit({ kind: "summaryDelta", id: "m1", text: "someone else's" });
    await emit({ kind: "summaryDelta", id: "m2", text: "Said hello." });
    // Streamed text is split into word spans: read the region's text.
    const notes = screen.getByRole("region", { name: "Notes" }).textContent;
    expect(notes).not.toContain("someone else's");
    expect(notes).toContain("Said hello.");

    stored[0].summary = "Said hello.";
    await act(async () => finishProcess({ id: "m2" }));
    await settle();
    expect(screen.queryByRole("status")).toBeNull();
    expect(within(screen.getByRole("region", { name: "Notes" })).getByText("Write again")).toBeTruthy();
  });

  test("a stop is not reported as a failure, a real failure is", async () => {
    render(<MeetingsPage openPane={() => {}} sideHandle={<span />} />);
    await settle();
    fireEvent.click(screen.getByText("Add recording"));
    await settle();
    fireEvent.click(screen.getByText("Stop"));
    expect(calls.at(-1)?.command).toBe("meeting_stop");
    await act(async () => finishProcess({ error: "stopped" }));
    await settle();
    expect(screen.queryByText("stopped")).toBeNull();

    fireEvent.click(screen.getByText("Add recording"));
    await settle();
    await act(async () => finishProcess({ error: "this recording cannot be read: no audio track" }));
    await settle();
    expect(screen.getByText("this recording cannot be read: no audio track")).toBeTruthy();
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

  test("times read as minutes, and hours past an hour", () => {
    expect([clock(5.4), clock(754), clock(3725)]).toEqual(["0:05", "12:34", "1:02:05"]);
  });
});
