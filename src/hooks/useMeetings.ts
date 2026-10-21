import { useCallback, useEffect, useRef, useState } from "react";
import { pickRecording } from "../lib/dialog";
import {
  meetingDelete,
  meetingGet,
  meetingJob,
  meetingList,
  meetingProcess,
  meetingRenameSpeaker,
  meetingStop,
  meetingSummarize,
  onMeetingEvent,
  type Meeting,
  type MeetingInfo,
  type MeetingJob,
} from "../lib/meeting";

export type Job = MeetingJob;

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

  // How many jobs were seen to end: a snapshot asked for before the last one
  // ended is of that job, and must not bring its card back.
  const ends = useRef(0);

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
      } else if (event.kind === "summaryDelta") {
        setDraft((now) => ({ id: event.id, text: (now?.id === event.id ? now.text : "") + event.text }));
      } else {
        ends.current += 1;
        setJob(null);
        setDraft(null);
        void readList();
        readOpen(openRef.current);
      }
    }).then((off) => {
      if (!live) return off();
      unlisten = off;
      // A job started before the page opened — it outlives the page. Read once
      // listening, so its end is not missed; what an event said wins.
      const seen = ends.current;
      meetingJob().then(
        (running) => live && ends.current === seen && setJob((now) => now ?? running),
        (e) => live && setError(String(e)),
      );
    });
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

  const importRecording = async () => {
    const path = await pickRecording();
    if (path) await run({ kind: "process" }, () => meetingProcess(path));
  };

  const summarize = (id: string) => run({ kind: "summary", id, stage: "notes" }, () => meetingSummarize(id));

  const stop = () => {
    stopping.current = true;
    void meetingStop();
  };

  const rename = async (speaker: number, name: string) => {
    const id = openRef.current;
    if (id === null) return;
    try {
      const renamed = await meetingRenameSpeaker(id, speaker, name);
      if (openRef.current === id) setMeeting(renamed);
    } catch (e) {
      setError(String(e));
    }
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
    importRecording,
    summarize,
    stop,
    rename,
    remove,
  };
}
