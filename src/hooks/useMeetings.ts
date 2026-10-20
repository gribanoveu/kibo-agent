import { useCallback, useEffect, useRef, useState } from "react";
import { pickRecording } from "../lib/dialog";
import {
  meetingDelete,
  meetingGet,
  meetingList,
  meetingProcess,
  meetingStop,
  meetingSummarize,
  onMeetingEvent,
  type Meeting,
  type MeetingInfo,
  type Stage,
} from "../lib/meeting";

/** What runs now: a recording being processed, or notes written again. */
export type Job = {
  kind: "process" | "summary";
  /** Known once the backend names it — at once for notes, on the first progress for a recording. */
  id?: string;
  name?: string;
  stage?: Stage | "notes";
  done?: number;
  total?: number;
};

/**
 * The Meetings page's state: the list, the meeting open, and the one job the
 * backend runs at a time. Events name their meeting; a piece of notes is kept
 * for the meeting it is about and shown only while that one is open.
 */
export function useMeetings() {
  const [list, setList] = useState<MeetingInfo[]>([]);
  const [openId, setOpenId] = useState<string | null>(null);
  const [meeting, setMeeting] = useState<Meeting | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const [draft, setDraft] = useState<{ id: string; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const stopping = useRef(false);
  // The job's meeting as the listener sees it, without re-subscribing per event.
  const jobRef = useRef<Job | null>(null);
  jobRef.current = job;

  const openRef = useRef<string | null>(null);
  openRef.current = openId;

  const readList = useCallback(() => meetingList().then(setList, (e) => setError(String(e))), []);
  // A read that lands after another meeting was opened is dropped.
  const readOpen = useCallback((id: string | null) => {
    if (id === null) return setMeeting(null);
    meetingGet(id).then(
      (m) => openRef.current === id && setMeeting(m),
      (e) => setError(String(e)),
    );
  }, []);

  useEffect(() => void readList(), []);
  useEffect(() => readOpen(openId), [openId]);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    onMeetingEvent((event) => {
      if (event.kind === "progress") {
        setJob((now) => now && { ...now, id: event.id, name: event.name, stage: event.stage, done: event.done, total: event.total });
      } else if (event.kind === "transcribed") {
        void readList();
        // The recording just added opens as soon as there is a transcript to read.
        if (jobRef.current?.id === event.id) setOpenId(event.id);
        setJob((now) => (now?.id === event.id ? { ...now, stage: "notes" } : now));
      } else {
        setDraft((now) => ({ id: event.id, text: (now?.id === event.id ? now.text : "") + event.text }));
      }
    }).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  const run = async (next: Job, work: () => Promise<unknown>) => {
    setError(null);
    stopping.current = false;
    setJob(next);
    try {
      await work();
    } catch (e) {
      if (!stopping.current) setError(String(e));
    } finally {
      setJob(null);
      setDraft(null);
      void readList();
      readOpen(openRef.current);
    }
  };

  const add = async () => {
    const path = await pickRecording();
    if (path) await run({ kind: "process" }, () => meetingProcess(path));
  };

  const summarize = (id: string) => run({ kind: "summary", id, stage: "notes" }, () => meetingSummarize(id));

  const stop = () => {
    stopping.current = true;
    void meetingStop();
  };

  const remove = async (id: string) => {
    try {
      await meetingDelete(id);
      if (openRef.current === id) setOpenId(null);
      await readList();
    } catch (e) {
      setError(String(e));
    }
  };

  return {
    list,
    meeting: meeting && meeting.id === openId ? meeting : null,
    openId,
    open: setOpenId,
    job,
    draft: draft && draft.id === openId ? draft.text : null,
    error,
    add,
    summarize,
    stop,
    remove,
  };
}
