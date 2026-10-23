import { useCallback, useEffect, useRef, useState } from "react";
import { pickRecording } from "../lib/dialog";
import {
  hearLevels,
  meetingAutoProcessGet,
  meetingAutoProcessSet,
  meetingDenoiseGet,
  meetingDenoiseSet,
  meetingDelete,
  meetingDeleteDocument,
  meetingGet,
  meetingJob,
  meetingList,
  meetingProcess,
  meetingProcessRecording,
  meetingRecordMute,
  meetingRecordStart,
  meetingRecordStop,
  meetingRecording,
  meetingRecordingDelete,
  meetingRecordings,
  meetingRenameSpeaker,
  meetingStop,
  meetingSummarize,
  meetingWriteDocument,
  onMeetingEvent,
  type Meeting,
  type MeetingInfo,
  type MeetingJob,
  type Recording,
  type RecordingInfo,
} from "../lib/meeting";

export type Job = MeetingJob;

/**
 * The Meetings page's state: the list, the meeting open, and the one job the
 * backend runs at a time. Events name their meeting; a piece of notes is kept
 * for the meeting it is about and shown only while that one is open. Beside
 * them, the call being recorded and the recorded ones waiting to be transcribed.
 * `first` is the meeting open to begin with — one another page asked for.
 */
export function useMeetings(first?: string) {
  const [list, setList] = useState<MeetingInfo[]>([]);
  const [openId, setOpenId] = useState<string | null>(first ?? null);
  const [meeting, setMeeting] = useState<Meeting | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  // What is being written now, for meeting `id`: its notes (`topic` null) or its document on `topic`.
  const [draft, setDraft] = useState<{ id: string; topic: string | null; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recordings, setRecordings] = useState<RecordingInfo[]>([]);
  const [recording, setRecording] = useState<Recording | null>(null);
  const [autoProcess, setAutoProcess] = useState(false);
  const [denoise, setDenoise] = useState(true);
  // Recordings to transcribe once the job running now ends, in order.
  const [queued, setQueued] = useState<string[]>([]);
  // The recording the job running now transcribes: the job is named after the meeting it makes, not the recording.
  const [transcribing, setTranscribing] = useState<string | null>(null);
  // Recordings stopped here: a level event about one that lands after the stop is late, not a new recording.
  const stoppedIds = useRef(new Set<string>());
  // A Record or Stop on its way: a second click is not a second call.
  const [switching, setSwitching] = useState(false);
  const stopping = useRef(false);
  // The job's meeting as the listener sees it, without re-subscribing per event.
  const jobRef = useRef<Job | null>(null);
  jobRef.current = job;

  const openRef = useRef<string | null>(null);
  openRef.current = openId;

  const readList = useCallback(
    () => Promise.all([meetingList().then(setList), meetingRecordings().then(setRecordings)]).catch((e) => setError(String(e))),
    [],
  );
  // A read that lands after another meeting was opened is dropped.
  const readOpen = useCallback((id: string | null) => {
    if (id === null) return setMeeting(null);
    meetingGet(id).then(
      (m) => openRef.current === id && setMeeting(m),
      (e) => setError(String(e)),
    );
  }, []);

  useEffect(() => void readList(), []);
  useEffect(() => void meetingAutoProcessGet().then(setAutoProcess, (e) => setError(String(e))), []);
  useEffect(() => void meetingDenoiseGet().then(setDenoise, (e) => setError(String(e))), []);
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
      } else if (event.kind === "recording") {
        if (!stoppedIds.current.has(event.id)) setRecording((now) => hearLevels(now, event));
      } else if (event.kind === "recordingEnded") {
        stoppedIds.current.add(event.id);
        setRecording((now) => (now?.id === event.id ? null : now));
        setError(event.error);
        void readList();
      } else if (event.kind === "summaryDelta" || event.kind === "documentDelta") {
        const topic = event.kind === "documentDelta" ? event.topic : null;
        setDraft((now) => ({ id: event.id, topic, text: (now?.id === event.id && now.topic === topic ? now.text : "") + event.text }));
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
      // A call being recorded since before the page opened; what a level event said wins.
      meetingRecording().then(
        (on) => live && on && setRecording((now) => now ?? { ...hearLevels(null, { ...on, mic: 0, call: 0 }), muted: on.muted }),
        (e) => live && setError(String(e)),
      );
    });
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  // Which call of `run` is the last: a snapshot read for an earlier one is stale.
  const runs = useRef(0);

  const run = async (next: Job, work: () => Promise<unknown>) => {
    const mine = ++runs.current;
    setError(null);
    stopping.current = false;
    setJob(next);
    try {
      await work();
    } catch (e) {
      if (!stopping.current) setError(String(e));
    } finally {
      // What runs now, read again rather than assumed gone: a call turned away
      // as busy ended nothing — another job runs, and its card and notes stay.
      meetingJob().then(
        (running) => runs.current === mine && setJob(running),
        (e) => setError(String(e)),
      );
      void readList();
      readOpen(openRef.current);
    }
  };

  const importRecording = async () => {
    const path = await pickRecording();
    if (path) await run({ kind: "process" }, () => meetingProcess(path));
  };

  const summarize = (id: string) => run({ kind: "summary", id, stage: "notes" }, () => meetingSummarize(id));
  const writeDocument = (id: string, topic: string) => run({ kind: "document", id, topic, stage: "notes" }, () => meetingWriteDocument(id, topic));

  // One job at a time: a recording asked for while one runs waits for it to end.
  const transcribeNow = (id: string) => {
    setTranscribing(id);
    void run({ kind: "process" }, () => meetingProcessRecording(id)).finally(() => setTranscribing((now) => (now === id ? null : now)));
  };
  const transcribe = (id: string) => {
    if (id === transcribing) return;
    if (jobRef.current !== null) setQueued((now) => (now.includes(id) ? now : [...now, id]));
    else transcribeNow(id);
  };
  // ponytail: the queue is the page's; one left when the page closes stays in the list for Transcribe.
  useEffect(() => {
    if (job !== null || queued.length === 0) return;
    const [next, ...rest] = queued;
    setQueued(rest);
    transcribeNow(next);
  }, [job, queued]);

  const record = async () => {
    setError(null);
    setSwitching(true);
    try {
      const id = await meetingRecordStart();
      setRecording((now) => now ?? hearLevels(null, { id, seconds: 0, mic: 0, call: 0 }));
    } catch (e) {
      setError(String(e));
    } finally {
      setSwitching(false);
    }
  };

  const stopRecording = async () => {
    const was = recording;
    if (was) stoppedIds.current.add(was.id);
    setSwitching(true);
    try {
      const id = await meetingRecordStop();
      stoppedIds.current.add(id);
      setRecording(null);
      await readList();
      if (autoProcess) transcribe(id);
    } catch (e) {
      setRecording(null);
      setError(String(e));
    } finally {
      setSwitching(false);
    }
  };

  const mute = async (on: boolean) => {
    try {
      await meetingRecordMute(on);
      setRecording((now) => now && { ...now, muted: on });
    } catch (e) {
      setError(String(e));
    }
  };

  const removeRecording = async (id: string) => {
    try {
      await meetingRecordingDelete(id);
      setQueued((now) => now.filter((q) => q !== id));
      await readList();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleAutoProcess = () => flip(autoProcess, setAutoProcess, meetingAutoProcessSet);
  const toggleDenoise = () => flip(denoise, setDenoise, meetingDenoiseSet);

  // A switch shown flipped at once, put back if saving it fails.
  async function flip(now: boolean, show: (on: boolean) => void, save: (on: boolean) => Promise<void>) {
    show(!now);
    try {
      await save(!now);
    } catch (e) {
      show(now);
      setError(String(e));
    }
  }

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

  const deleteDocument = async (topic: string) => {
    const id = openRef.current;
    if (id === null) return;
    try {
      const after = await meetingDeleteDocument(id, topic);
      if (openRef.current === id) setMeeting(after);
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
    draft: draft && draft.id === openId ? { topic: draft.topic, text: draft.text } : null,
    error,
    clearError: () => setError(null),
    importRecording,
    summarize,
    writeDocument,
    deleteDocument,
    recordings,
    recording,
    switching,
    queued,
    transcribing,
    autoProcess,
    denoise,
    record,
    stopRecording,
    mute,
    transcribe,
    removeRecording,
    toggleAutoProcess,
    toggleDenoise,
    stop,
    rename,
    remove,
  };
}
