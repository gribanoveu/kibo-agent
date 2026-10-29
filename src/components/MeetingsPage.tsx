import { useEffect, useRef, useState } from "react";
import { matches } from "../lib/shortcuts";
import { windowsFrame } from "../lib/window";
import { AudioLines, Calendar, Volume2, Check, ChevronLeft, ChevronRight, Clock, Copy, FileAudio, FilePlus, FileText, HardDrive, Hourglass, LoaderCircle, MessageSquareText, Mic, MicOff, Pause, Play, Plus, RefreshCw, Settings2, Sparkles, Square, Trash2, Users, X } from "lucide-react";
import { Page, PageSheet } from "./Page";
import { Markdown } from "./Markdown";
import { ErrorNote } from "./ErrorNote";
import { Modal } from "./Modal";
import { Switch } from "./Switch";
import { Dropdown } from "./Dropdown";
import { useMeetings, type Job } from "../hooks/useMeetings";
import { sizeLabel } from "../lib/prototype";
import { useCopied } from "../hooks/useCopied";
import { useFeatures } from "../hooks/useFeatures";
import { useVoices } from "../hooks/useVoices";
import { usePlayer, type Player } from "../hooks/usePlayer";
import { useRecordingAccess } from "../hooks/useRecordingAccess";
import {
  audioUrl,
  callSilent,
  meetingTitle,
  speakerCount,
  speakerName,
  withoutTitle,
  withoutTopics,
  type Meeting,
  type MeetingDocument,
  type MeetingInfo,
  type Microphone,
  type Recording,
  type RecordingAccess,
  type RecordingInfo,
  type Source,
  type Stage,
  type VoiceInfo,
} from "../lib/meeting";
import type { PageContext } from "./pages";
import "./MeetingsPage.css";

const STAGES: Record<Stage, string> = {
  decode: "Reading the recording",
  speech: "Recognizing speech",
  speakers: "Telling the voices apart",
  notes: "Writing the notes",
};
/** In the order they run: the progress card's segments. */
const STEPS: Stage[] = ["decode", "speech", "speakers", "notes"];

/** A speaker's colour class: six, then round again. */
const tone = (speaker: number) => `speaker-${speaker % 6}`;

/** `m:ss`, or `h:mm:ss` past an hour. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const mm = String(Math.floor(s / 60) % 60).padStart(s >= 3600 ? 2 : 1, "0");
  const ss = String(s % 60).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${mm}:${ss}` : `${mm}:${ss}`;
}

/** The list's heading for the day `created` falls on: Today, Yesterday, or the date. */
export function day(created: string, now = new Date()): string {
  const date = new Date(created);
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const ago = Math.round((midnight(now) - midnight(date)) / 86_400_000);
  if (ago === 0) return "Today";
  if (ago === 1) return "Yesterday";
  const year = date.getFullYear() === now.getFullYear() ? undefined : "numeric";
  return date.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "long", year });
}

const time = (created: string) => new Date(created).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

const when = (created: string) =>
  new Date(created).toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

/** A call's recording in, its notes out: the summary first, the transcript
    with speakers and times under it. Recognition runs on this computer; the
    notes are written by the model chosen in Settings. */
export function MeetingsPage({ item }: PageContext) {
  const meetings = useMeetings(item);
  const { job, meeting, openId } = meetings;
  // A meeting, or a recording not yet transcribed — the only copy of its call.
  const [deleting, setDeleting] = useState<{ id: string; title: string; recording: boolean } | null>(null);
  const remembering = useFeatures().isOn("voice-memory");
  const [voicesOpen, setVoicesOpen] = useState(false);
  const player = usePlayer(openId && audioUrl(openId));
  const access = useRecordingAccess();

  const bar = (
    <div className="meetings-head">
      <div className="meetings-bar">
        {openId && (
          <button type="button" className="btn btn-ghost" onClick={() => meetings.open(null)}>
            <ChevronLeft size={14} />
            All meetings
          </button>
        )}
        {!openId && (
          <button
            type="button"
            className="btn btn-primary meetings-record"
            disabled={meetings.recording !== null || meetings.switching || !access.ready}
            title={access.ready ? "Record a call: your microphone and what the computer plays" : "Allow the microphone and system audio first"}
            onClick={() => void meetings.record()}
          >
            <Mic size={14} />
            Record
          </button>
        )}
        {!openId && (
          <RecordingOptions
            autoProcess={meetings.autoProcess}
            denoise={meetings.denoise}
            microphone={meetings.microphone}
            microphones={meetings.microphones}
            onAutoProcess={() => void meetings.toggleAutoProcess()}
            onDenoise={() => void meetings.toggleDenoise()}
            onOpen={meetings.readMicrophones}
            onMicrophone={(id) => void meetings.pickMicrophone(id)}
          />
        )}
        {!openId && (
          <button type="button" className="btn btn-ghost" disabled={job !== null} onClick={() => void meetings.importRecording()}>
            <Plus size={14} />
            Import recording
          </button>
        )}
        {meeting && <PlayerBar player={player} seconds={meeting.seconds} />}
        {remembering && (
          <button type="button" className="btn btn-ghost meetings-voices-toggle" data-sheet-opener aria-pressed={voicesOpen} onClick={() => setVoicesOpen(!voicesOpen)}>
            <Users size={14} />
            Voices
          </button>
        )}
      </div>
    </div>
  );

  // Voices change when a speaker is named (the meeting is read again) or a meeting is deleted (the list is).
  const panel = remembering && voicesOpen && <VoicesPanel changed={meeting ?? meetings.list} onClose={() => setVoicesOpen(false)} />;

  return (
    <Page title="Meetings" bar={bar} sheet={panel}>
      {openId && <audio {...player.audio} />}
      {!openId && access.access && <AccessNotes access={access.access} onAllow={(source) => void access.allow(source)} />}
      {access.error && <ErrorNote className="meetings-error" message={access.error} onClose={access.clearError} />}
      {meetings.recording && (
        <RecorderCard
          recording={meetings.recording}
          stopping={meetings.switching}
          onMute={(on) => void meetings.mute(on)}
          onStop={() => void meetings.stopRecording()}
        />
      )}
      {job && <JobCard job={job} title={titleOf(job, meetings.list)} onStop={meetings.stop} />}
      {meetings.error && <ErrorNote className="meetings-error" message={meetings.error} onClose={meetings.clearError} />}
      {openId === null ? (
        <MeetingList
          list={meetings.list}
          recordings={meetings.recordings}
          queued={meetings.queued}
          transcribing={meetings.transcribing}
          onOpen={meetings.open}
          onDelete={(m) => setDeleting({ id: m.id, title: meetingTitle(m), recording: false })}
          onTranscribe={meetings.transcribe}
          onDeleteRecording={(r) => setDeleting({ id: r.id, title: recordedTitle, recording: true })}
          job={job}
        />
      ) : (
        meeting && (
          <MeetingView
            meeting={meeting}
            draft={meetings.draft}
            writing={job?.id === meeting.id && job.stage === "notes" ? (job.kind === "document" ? (job.topic ?? null) : NOTES) : null}
            busy={job !== null}
            onSummarize={() => void meetings.summarize(meeting.id)}
            onWriteDocument={(topic) => void meetings.writeDocument(meeting.id, topic)}
            onDeleteDocument={(topic) => void meetings.deleteDocument(topic)}
            onRename={(speaker, name) => void meetings.rename(speaker, name)}
            player={player}
            bytes={meetings.list.find((m) => m.id === meeting.id)?.bytes ?? 0}
          />
        )
      )}
      <Modal
        title={`Delete ${deleting?.title ?? ""}?`}
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDeleting(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (deleting?.recording) void meetings.removeRecording(deleting.id);
                else if (deleting) void meetings.remove(deleting.id);
                setDeleting(null);
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p className="meetings-confirm">
          {deleting?.recording
            ? "The recording goes, and with it the only copy of this call."
            : "Its transcript, notes and the app's copy of the recording go. The file it was imported from stays where it is."}
        </p>
      </Modal>
    </Page>
  );
}

const SOURCES: Source[] = ["microphone", "call"];
const SOURCE_TEXT: Record<Source, { name: string; title: string; why: string }> = {
  microphone: {
    name: "Microphone",
    title: "Allow the microphone — your side of the call",
    why: "macOS asks once. Your voice is recorded only between Record and Stop.",
  },
  call: {
    name: "System audio",
    title: "Allow system audio — the call's side",
    why: "What the computer plays, so the others on the call are recorded too.",
  },
};

/** A note for each side the system has not let the app record yet: nothing when both
    are allowed, or where it does not say. */
function AccessNotes({ access, onAllow }: { access: RecordingAccess; onAllow: (source: Source) => void }) {
  const missing = SOURCES.filter((s) => access[s] === "denied" || access[s] === "notAsked");
  return missing.map((source) => <AccessNote key={source} source={source} denied={access[source] === "denied"} onAllow={() => onAllow(source)} />);
}

/** Where a refused side is turned on. Windows refuses only the microphone, and asks a
    desktop app nothing: its notes are always that one, refused. */
const SETTINGS = windowsFrame
  ? { name: "Settings", how: "Turned off in Settings → Privacy & security → Microphone — turn on access for desktop apps there." }
  : { name: "System Settings", how: "Turned off in System Settings — switch Kibo Agent on there." };

/** One side not allowed, and the way to allow it: the system's prompt, or its settings
    for one refused — no prompt asks that again. */
function AccessNote({ source, denied, onAllow }: { source: Source; denied: boolean; onAllow: () => void }) {
  const text = SOURCE_TEXT[source];
  return (
    <div className="meetings-access" role="note" aria-label={`${text.name} permission`}>
      <span className="meetings-access-icon" aria-hidden>
        {source === "microphone" ? <Mic size={16} /> : <Volume2 size={16} />}
      </span>
      <div className="meetings-access-body">
        <p className="meetings-access-title">{text.title}</p>
        <p className="meetings-access-state">{denied ? SETTINGS.how : text.why}</p>
      </div>
      <button type="button" className="btn btn-ghost" onClick={onAllow}>
        {denied ? `Open ${SETTINGS.name}` : "Allow"}
      </button>
    </div>
  );
}

/** The menu's value for the system's default microphone: no id is empty. */
const DEFAULT_MIC = "";

/** The recording's settings, set once and left: behind a gear beside Record, not on the page. */
function RecordingOptions({
  autoProcess,
  denoise,
  microphone,
  microphones,
  onAutoProcess,
  onDenoise,
  onOpen,
  onMicrophone,
}: {
  autoProcess: boolean;
  denoise: boolean;
  microphone: string | null;
  microphones: Microphone[] | null;
  onAutoProcess: () => void;
  onDenoise: () => void;
  onOpen: () => void;
  onMicrophone: (id: string | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => matches(e, "close") && setOpen(false);
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="meetings-settings" ref={wrap}>
      <button
        type="button"
        className="btn btn-ghost meetings-settings-toggle"
        aria-label="Recording settings"
        title="Recording settings"
        aria-expanded={open}
        onClick={() => {
          if (!open) onOpen();
          setOpen(!open);
        }}
      >
        <Settings2 size={14} />
      </button>
      {open && (
        <div className="meetings-options" role="dialog" aria-label="Recording settings">
          <div className="meetings-mic" title="Your side of the call, from the next recording on; while the one chosen is unplugged or will not open, the system's default records instead">
            <Mic size={14} />
            <Dropdown
              below
              label={micLabel(microphone, microphones)}
              heading="Record from"
              value={microphone ?? DEFAULT_MIC}
              onPick={(id) => onMicrophone(id === DEFAULT_MIC ? null : id)}
              options={[
                { value: DEFAULT_MIC, label: "System default" },
                ...(microphones ?? []).map((m) => ({ value: m.id, label: m.name })),
                ...(microphone !== null && !microphones?.some((m) => m.id === microphone)
                  ? [{ value: microphone, label: micLabel(microphone, microphones) }]
                  : []),
              ]}
            />
          </div>
          <label className="meetings-auto" title="A recording is made into a meeting as soon as you press Stop">
            <Switch label="Transcribe when a recording stops" on={autoProcess} onChange={onAutoProcess} />
            Transcribe when a recording stops
          </label>
          <label className="meetings-auto" title="Background noise on your microphone is reduced before the call is transcribed; the recording itself stays as it was">
            <Switch label="Reduce noise on my side before transcribing" on={denoise} onChange={onDenoise} />
            Reduce noise on my side before transcribing
          </label>
        </div>
      )}
    </div>
  );
}

/** The trigger's words for the microphone chosen; one not plugged in now says so, once the list is read. */
function micLabel(microphone: string | null, microphones: Microphone[] | null): string {
  if (microphone === null) return "System default microphone";
  if (microphones === null) return "Chosen microphone";
  return microphones.find((m) => m.id === microphone)?.name ?? "Chosen microphone, not plugged in";
}

/** What a recording not yet transcribed is called in the list. */
const recordedTitle = "Recorded call";

/** The call being recorded: how long, how loud each side is, your microphone's Mute, and Stop. */
function RecorderCard({
  recording,
  stopping,
  onMute,
  onStop,
}: {
  recording: Recording;
  stopping: boolean;
  onMute: (on: boolean) => void;
  onStop: () => void;
}) {
  return (
    <div className="meetings-recorder" role="status" aria-label="Recording">
      <span className="meetings-recorder-dot" aria-hidden />
      <div className="meetings-recorder-body">
        <div className="meetings-job-head">
          <span className="meetings-job-title">Recording</span>
          <span className="meetings-recorder-time">{clock(recording.seconds)}</span>
        </div>
        <Meter label="You" level={recording.mic} muted={recording.muted} />
        <Meter label="Call" level={recording.call} />
        {/* Windows' loopback asks no leave: a call silent there is a quiet one. */}
        {!windowsFrame && callSilent(recording) && (
          <p className="meetings-recorder-hint">No sound from the call — check the System Audio Recording permission in System Settings → Privacy & Security.</p>
        )}
      </div>
      <button
        type="button"
        className="btn btn-ghost meetings-mute"
        aria-pressed={recording.muted}
        title={recording.muted ? "Your microphone is recorded as silence — click to be heard again" : "Record silence for your side for now — noise around you stays out"}
        onClick={() => onMute(!recording.muted)}
      >
        {recording.muted ? <MicOff size={12} /> : <Mic size={12} />}
        {recording.muted ? "Unmute" : "Mute"}
      </button>
      <button type="button" className="btn btn-ghost" disabled={stopping} onClick={onStop}>
        <Square size={12} />
        Stop
      </button>
    </div>
  );
}

/** One side's level, 0 to 1; `muted` says so instead. */
function Meter({ label, level, muted = false }: { label: string; level: number; muted?: boolean }) {
  return (
    <div className="meetings-meter">
      <span className="meetings-meter-label">{label}</span>
      {muted && <span className="meetings-meter-muted">Muted</span>}
      <span className="meetings-meter-track" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={1} aria-valuenow={level}>
        {/* A square root: speech peaks low, and a meter that barely moves says nothing. */}
        <span className="meetings-meter-fill" style={{ width: `${Math.round(100 * Math.sqrt(Math.min(1, level)))}%` }} />
      </span>
    </div>
  );
}

/** The open meeting's recording: play or pause, where it is, and a scale to move along. */
function PlayerBar({ player, seconds }: { player: Player; seconds: number }) {
  if (player.missing) return <span className="meetings-player-missing">No recording kept for this meeting</span>;
  const length = player.duration || seconds;
  return (
    <div className="meetings-player" aria-label="Recording">
      <button type="button" className="meetings-player-play" aria-label={player.playing ? "Pause" : "Play"} onClick={player.toggle}>
        {player.playing ? <Pause size={14} /> : <Play size={14} />}
      </button>
      <span className="meetings-player-time">{clock(player.time)}</span>
      <input
        type="range"
        aria-label="Position"
        min={0}
        max={length}
        step={0.1}
        value={Math.min(player.time, length)}
        style={{ "--fill": `${length ? (100 * player.time) / length : 0}%` } as React.CSSProperties}
        onChange={(e) => player.seek(Number(e.currentTarget.value))}
      />
      <span className="meetings-player-time">{clock(length)}</span>
    </div>
  );
}

/** What the job is about: the meeting's title once it is listed, else the recording's file. */
const titleOf = (job: Job, list: MeetingInfo[]) => {
  const listed = list.find((m) => m.id === job.id);
  return listed ? meetingTitle(listed) : (job.name ?? "Recording");
};

/** How far one stage is, 0 to 1, or `null` when it cannot say. */
function share({ stage, done = 0, total = 0 }: Job): number | null {
  if (!total || stage === "decode") return null;
  // The notes of a long meeting's parts count; the notes of the whole are one stream.
  if (stage === "notes" && done >= total) return null;
  return Math.min(1, done / total);
}

/** The job running now: what it works on, the stage and how far along, one
    segment a stage, and Stop. */
function JobCard({ job, title, onStop }: { job: Job; title: string; onStop: () => void }) {
  const { stage, done = 0, total = 0 } = job;
  const at = stage ? STEPS.indexOf(stage) : -1;
  // Every voice embedded, the clustering runs and says nothing until done.
  const grouping = stage === "speakers" && total > 0 && done >= total;
  const part = grouping ? null : share(job);
  const label = grouping ? "Grouping the voices" : job.topic ? `Writing up “${job.topic}”` : stage ? STAGES[stage] : "Starting";
  const detail = part === null ? "" : stage === "notes" ? ` part ${done + 1} of ${total}` : ` ${Math.round(100 * part)}%`;
  return (
    <div className="meetings-job" role="status">
      <span className="meetings-job-icon" aria-hidden>
        <LoaderCircle size={16} />
      </span>
      <div className="meetings-job-body">
        <div className="meetings-job-head">
          <span className="meetings-job-title">{title}</span>
          {at >= 0 && (
            <span className="meetings-job-step">
              Step {at + 1} of {STEPS.length}
            </span>
          )}
        </div>
        <div className="meetings-job-stage">
          {label}…{detail}
        </div>
        <div className="meetings-job-track">
          {STEPS.map((step, i) => (
            <span key={step} className="meetings-job-segment" title={STAGES[step]}>
              <span
                className={`meetings-job-fill${i === at && part === null ? " busy" : ""}`}
                style={{ width: `${i < at ? 100 : i === at ? 100 * (part ?? 1) : 0}%` }}
              />
            </span>
          ))}
        </div>
      </div>
      <button type="button" className="btn btn-ghost" onClick={onStop}>
        <Square size={12} />
        Stop
      </button>
    </div>
  );
}

/** Whether a meeting is done: notes written, only its transcript, or notes being written now. */
function MeetingState({ summarized, writing }: { summarized: boolean; writing: boolean }) {
  if (writing)
    return (
      <span className="meetings-state writing">
        <LoaderCircle size={11} />
        Writing notes
      </span>
    );
  return summarized ? (
    <span className="meetings-state done">
      <Sparkles size={11} />
      Notes
    </span>
  ) : (
    <span className="meetings-state">
      <FileText size={11} />
      Transcript only
    </span>
  );
}

function MeetingList({
  list,
  recordings,
  queued,
  transcribing,
  onOpen,
  onDelete,
  onTranscribe,
  onDeleteRecording,
  job,
}: {
  list: MeetingInfo[];
  recordings: RecordingInfo[];
  queued: string[];
  transcribing: string | null;
  onOpen: (id: string) => void;
  onDelete: (m: MeetingInfo) => void;
  onTranscribe: (id: string) => void;
  onDeleteRecording: (r: RecordingInfo) => void;
  job: Job | null;
}) {
  const busy = job !== null;
  if (list.length === 0 && recordings.length === 0) {
    return (
      <p className="meetings-empty">
        No meetings yet. Record a call, or import its recording — sound or video — and its notes are written here: what was decided, who does what.
      </p>
    );
  }
  // Both newest first, merged: a day's rows are together.
  const rows: { created: string; meeting?: MeetingInfo; recording?: RecordingInfo }[] = [
    ...list.map((m) => ({ created: m.created, meeting: m })),
    ...recordings.map((r) => ({ created: r.created, recording: r })),
  ].sort((a, b) => Date.parse(b.created) - Date.parse(a.created));
  const days: [string, typeof rows][] = [];
  for (const row of rows) {
    const label = day(row.created);
    const last = days[days.length - 1];
    if (last?.[0] === label) last[1].push(row);
    else days.push([label, [row]]);
  }
  return (
    <div className="meetings-days">
      {days.map(([label, rows]) => (
        <section key={label} className="meetings-day" aria-label={label}>
          <h3 className="meetings-day-head">{label}</h3>
          <ul className="meetings-list">
            {rows.map((row) =>
              row.meeting ? (
                <MeetingRow key={row.meeting.id} m={row.meeting} job={job} busy={busy} onOpen={onOpen} onDelete={onDelete} />
              ) : (
                row.recording && (
                  <RecordingRow
                    key={row.recording.id}
                    r={row.recording}
                    queued={queued.includes(row.recording.id)}
                    transcribing={row.recording.id === transcribing}
                    onTranscribe={onTranscribe}
                    onDelete={onDeleteRecording}
                  />
                )
              ),
            )}
          </ul>
        </section>
      ))}
    </div>
  );
}

function MeetingRow({ m, job, busy, onOpen, onDelete }: { m: MeetingInfo; job: Job | null; busy: boolean; onOpen: (id: string) => void; onDelete: (m: MeetingInfo) => void }) {
  return (
    <li className="meetings-row">
      <button type="button" className="meetings-row-open" onClick={() => onOpen(m.id)}>
        <span className="meetings-row-icon" aria-hidden>
          <AudioLines size={16} />
        </span>
        <span className="meetings-row-body">
          <span className="meetings-row-name">{meetingTitle(m)}</span>
          <span className="meetings-row-meta">
            <span>{time(m.created)}</span>
            <span title="Duration">
              <Clock size={11} />
              {clock(m.seconds)}
            </span>
            <span title="Speakers">
              <Users size={11} />
              {m.speakers}
            </span>
          </span>
        </span>
        <MeetingState summarized={m.summarized} writing={job?.id === m.id && job.stage === "notes"} />
      </button>
      <button type="button" className="meetings-row-delete" title="Delete" aria-label={`Delete ${meetingTitle(m)}`} disabled={busy} onClick={() => onDelete(m)}>
        <Trash2 size={14} />
      </button>
    </li>
  );
}

/** A call recorded and not yet transcribed: Transcribe makes it a meeting, or waits for the job running now. Being transcribed, or waiting, it is not deleted. */
function RecordingRow({
  r,
  queued,
  transcribing,
  onTranscribe,
  onDelete,
}: {
  r: RecordingInfo;
  queued: boolean;
  transcribing: boolean;
  onTranscribe: (id: string) => void;
  onDelete: (r: RecordingInfo) => void;
}) {
  return (
    <li className="meetings-row">
      <div className="meetings-row-open meetings-row-recording">
        <span className="meetings-row-icon" aria-hidden>
          <Mic size={16} />
        </span>
        <span className="meetings-row-body">
          <span className="meetings-row-name">{recordedTitle}</span>
          <span className="meetings-row-meta">
            <span>{time(r.created)}</span>
            <span title="Duration">
              <Clock size={11} />
              {clock(r.seconds)}
            </span>
          </span>
        </span>
        {transcribing ? (
          <span className="meetings-state writing">
            <LoaderCircle size={11} />
            Transcribing
          </span>
        ) : queued ? (
          <span className="meetings-state">
            <Hourglass size={11} />
            Waiting
          </span>
        ) : (
          <>
            <span className="meetings-state">
              <FileText size={11} />
              Not transcribed
            </span>
            <button type="button" className="btn btn-ghost" onClick={() => onTranscribe(r.id)}>
              <Sparkles size={12} />
              Transcribe
            </button>
          </>
        )}
      </div>
      <button type="button" className="meetings-row-delete" title="Delete" aria-label={`Delete ${recordedTitle} of ${time(r.created)}`} disabled={queued || transcribing} onClick={() => onDelete(r)}>
        <Trash2 size={14} />
      </button>
    </li>
  );
}

/** What `writing` names when the notes are being written, not a document. */
const NOTES = Symbol("notes");

function MeetingView({
  meeting,
  draft,
  writing: what,
  busy,
  onSummarize,
  onWriteDocument,
  onDeleteDocument,
  onRename,
  player,
  bytes,
}: {
  meeting: Meeting;
  /** What is streaming in: the notes (`topic` null) or a document. */
  draft: { topic: string | null; text: string } | null;
  /** What is being written now: the notes, the document on a subject, or nothing. */
  writing: typeof NOTES | string | null;
  busy: boolean;
  onSummarize: () => void;
  onWriteDocument: (topic: string) => void;
  onDeleteDocument: (topic: string) => void;
  onRename: (speaker: number, name: string) => void;
  player: Player;
  /** Its kept recording's size on disk, from the list; 0 when it has none. */
  bytes: number;
}) {
  const { copied, copy } = useCopied();
  const writing = what === NOTES;
  const notes = writing ? withoutTopics(withoutTitle(draft?.topic === null ? draft.text : "")) : meeting.summary;
  // The documents, and the one being written first time after them.
  const written = meeting.documents ?? [];
  const documents = typeof what === "string" && !written.some((d) => d.topic === what) ? [...written, { topic: what, text: "" }] : written;
  const suggested = (meeting.topics ?? []).filter((t) => !documents.some((d) => d.topic === t));
  const speakers = speakerCount(meeting);
  return (
    <article className="meeting" aria-label={meetingTitle(meeting)}>
      <header className="meeting-head">
        <h2>{meetingTitle(meeting)}</h2>
        <div className="meeting-facts">
          <span title="Recognized">
            <Calendar size={13} />
            {when(meeting.created)}
          </span>
          <span title="Duration">
            <Clock size={13} />
            {clock(meeting.seconds)}
          </span>
          <span title="Speakers">
            <Users size={13} />
            {speakers} {speakers === 1 ? "speaker" : "speakers"}
          </span>
          {bytes > 0 && (
            <span title="Size of the recording">
              <HardDrive size={13} />
              {sizeLabel(bytes)}
            </span>
          )}
          {meeting.title && (
            <span title="Imported from">
              <FileAudio size={13} />
              {meeting.name}
            </span>
          )}
        </div>
      </header>

      <div className="meeting-cap" aria-hidden />
      <MeetingCard
        title="Notes"
        icon={<Sparkles size={14} className="meeting-card-icon" />}
        actions={
          <>
            {notes && !writing && (
              <button type="button" className="btn btn-ghost" onClick={() => copy(notes)}>
                <Copy size={12} />
                {copied ? "Copied" : "Copy"}
              </button>
            )}
            <button type="button" className="btn btn-ghost" disabled={busy} onClick={onSummarize}>
              <RefreshCw size={12} />
              {meeting.summary ? "Write again" : "Write notes"}
            </button>
          </>
        }
      >
        <div className="meeting-notes">
          {notes ? (
            <Markdown text={notes} streaming={writing} />
          ) : (
            <p className="meetings-empty">{writing ? "Writing…" : "No notes yet — the model may have failed or been stopped."}</p>
          )}
        </div>
        {suggested.length > 0 && !writing && (
          <div className="meeting-topics" aria-label="Worth a document">
            <span className="meeting-topics-label">
              <FilePlus size={13} />
              Write up
            </span>
            {suggested.map((topic) => (
              <button key={topic} type="button" className="meeting-topic" disabled={busy} title="Write a document on everything the meeting said about this" onClick={() => onWriteDocument(topic)}>
                {topic}
              </button>
            ))}
          </div>
        )}
      </MeetingCard>

      {documents.map((d) => (
        <DocumentCard
          key={d.topic}
          document={d}
          draft={what === d.topic ? (draft?.topic === d.topic ? draft.text : "") : null}
          busy={busy}
          onWrite={() => onWriteDocument(d.topic)}
          onDelete={() => onDeleteDocument(d.topic)}
        />
      ))}

      <MeetingCard title="Transcript" icon={<MessageSquareText size={14} className="meeting-card-icon" />}>
        <Speakers meeting={meeting} onRename={onRename} />
        <div className="meeting-transcript">
          {turns(meeting).map((turn, i, all) => {
            // The turn the player is in, once it has started.
            const now = player.time > 0 && player.time >= turn.start && player.time < (all[i + 1]?.start ?? Infinity);
            return (
              <div key={turn.start} className={`meeting-turn${now ? " now" : ""}`}>
                <button
                  type="button"
                  className={`meeting-avatar ${tone(turn.speaker)}`}
                  aria-label={`Play from ${clock(turn.start)}`}
                  title={`Play from ${clock(turn.start)}`}
                  disabled={player.missing}
                  onClick={() => (now && player.playing ? player.toggle() : player.playFrom(turn.start))}
                >
                  <span className="meeting-avatar-letter">{initial(meeting, turn.speaker)}</span>
                  {now && player.playing ? <Pause size={12} className="meeting-avatar-icon" /> : <Play size={12} className="meeting-avatar-icon" />}
                </button>
                <div className="meeting-turn-body">
                  <div className="meeting-turn-head">
                    <span className={`meeting-speaker ${tone(turn.speaker)}`}>{speakerName(meeting, turn.speaker)}</span>
                    <span className="meeting-time">{clock(turn.start)}</span>
                  </div>
                  {turn.texts.map((text, i) => (
                    <p key={i} className="meeting-text">
                      {text}
                    </p>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      </MeetingCard>
    </article>
  );
}

/** A document written from the meeting, shown as its notes are; `draft` is its text streaming in while it is written. */
function DocumentCard({
  document,
  draft,
  busy,
  onWrite,
  onDelete,
}: {
  document: MeetingDocument;
  draft: string | null;
  busy: boolean;
  onWrite: () => void;
  onDelete: () => void;
}) {
  const { copied, copy } = useCopied();
  const writing = draft !== null;
  const text = writing ? draft : document.text;
  return (
    <MeetingCard
      title={document.topic}
      icon={<FileText size={14} className="meeting-card-icon" />}
      actions={
        <>
          {text && !writing && (
            <button type="button" className="btn btn-ghost" onClick={() => copy(text)}>
              <Copy size={12} />
              {copied ? "Copied" : "Copy"}
            </button>
          )}
          <button type="button" className="btn btn-ghost" disabled={busy} onClick={onWrite}>
            <RefreshCw size={12} />
            Write again
          </button>
          <button type="button" className="btn btn-ghost" aria-label={`Delete ${document.topic}`} title="Delete" disabled={busy} onClick={onDelete}>
            <Trash2 size={12} />
          </button>
        </>
      }
    >
      <div className="meeting-notes">{text ? <Markdown text={text} streaming={writing} /> : <p className="meetings-empty">Writing…</p>}</div>
    </MeetingCard>
  );
}

/** A card of the open meeting: its title folds it to that one line, its actions stay beside it. */
function MeetingCard({ title, icon, actions, children }: { title: string; icon: React.ReactNode; actions?: React.ReactNode; children: React.ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <section className="meeting-card" aria-label={title}>
      <div className="meeting-card-head">
        <h3>
          <button type="button" className="meeting-card-fold" aria-expanded={open} title={open ? "Fold" : "Unfold"} onClick={() => setOpen(!open)}>
            <ChevronRight size={13} className="meeting-card-chev" />
            {icon}
            {title}
          </button>
        </h3>
        {actions}
      </div>
      {open && children}
    </section>
  );
}

/** The transcript as turns: one speaker's utterances in a row are one turn. */
function turns(meeting: Meeting): { speaker: number; start: number; texts: string[] }[] {
  const out: { speaker: number; start: number; texts: string[] }[] = [];
  for (const u of meeting.utterances) {
    const last = out[out.length - 1];
    if (last?.speaker === u.speaker) last.texts.push(u.text);
    else out.push({ speaker: u.speaker, start: u.start, texts: [u.text] });
  }
  return out;
}

/** A named speaker's first letter, an unnamed one's number. */
const initial = (meeting: Meeting, speaker: number) => meeting.names?.[speaker]?.trim().charAt(0).toUpperCase() || String(speaker + 1);

/** Who spoke, each renamed in place: the name goes into the transcript and the
    notes. Two given one name are one person the recognition split in two. */
function Speakers({ meeting, onRename }: { meeting: Meeting; onRename: (speaker: number, name: string) => void }) {
  const [editing, setEditing] = useState<number | null>(null);
  return (
    <div className="meeting-speakers" aria-label="Speakers" title="Click a name to rename the speaker">
      {Array.from({ length: speakerCount(meeting) }, (_, speaker) =>
        editing === speaker ? (
          <NameEditor
            key={speaker}
            speaker={speaker}
            name={meeting.names?.[speaker] ?? ""}
            onSave={(name) => {
              onRename(speaker, name);
              setEditing(null);
            }}
            onClose={() => setEditing(null)}
          />
        ) : (
          <button
            key={speaker}
            type="button"
            className={`meeting-speaker-chip ${tone(speaker)}${meeting.guessed?.includes(speaker) ? " guessed" : ""}`}
            title={meeting.guessed?.includes(speaker) ? "Recognized by voice — click to keep or correct" : "Rename"}
            onClick={() => setEditing(speaker)}
          >
            {speakerName(meeting, speaker)}
          </button>
        ),
      )}
    </div>
  );
}

/** A speaker's name being typed. Enter or ✓ saves it — a name the voice memory
    guessed is kept so, unchanged; clicking away saves it only if it changed;
    Escape leaves it as it was. × empties the field: saved empty, the speaker
    is its number again. */
function NameEditor({ speaker, name, onSave, onClose }: { speaker: number; name: string; onSave: (name: string) => void; onClose: () => void }) {
  const [value, setValue] = useState(name);
  // Enter, ✓ and Escape each end the edit, and the blur that follows must not end it again.
  const ended = useRef(false);
  const end = (save: boolean) => {
    if (ended.current) return;
    ended.current = true;
    if (save) onSave(value);
    else onClose();
  };
  // The buttons take no focus: the field keeps it, and its blur is not a click away.
  const keepFocus = (e: React.MouseEvent) => e.preventDefault();
  return (
    <span className="meeting-speaker-edit">
      <input
        className="meeting-speaker-input"
        aria-label={`Name of speaker ${speaker + 1}`}
        placeholder={`Speaker ${speaker + 1}`}
        value={value}
        maxLength={60}
        autoFocus
        onChange={(e) => setValue(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (matches(e.nativeEvent, "accept")) end(true);
          else if (matches(e.nativeEvent, "close")) end(false);
        }}
        onBlur={() => end(value.trim() !== name.trim())}
      />
      {value && (
        <button type="button" className="meeting-speaker-tool" aria-label="Clear" title="Clear" onMouseDown={keepFocus} onClick={() => setValue("")}>
          <X size={12} />
        </button>
      )}
      <button type="button" className="meeting-speaker-tool accept" aria-label="Save name" title="Save" onMouseDown={keepFocus} onClick={() => end(true)}>
        <Check size={12} />
      </button>
    </span>
  );
}

/** The voice memory's voices, each one forgotten on request — its every sample
    goes; the names already given in meetings stay as text. */
function VoicesPanel({ changed, onClose }: { changed: unknown; onClose: () => void }) {
  const { voices, error, clearError, remove } = useVoices(changed);
  const [forgetting, setForgetting] = useState<VoiceInfo | null>(null);
  return (
    <PageSheet icon={Users} title="Voices" label="Voices" onClose={onClose}>
      {error && <ErrorNote className="meetings-error" message={error} onClose={clearError} />}
      {voices.length === 0 ? (
        <p className="meetings-empty">
          No voices yet. Name a speaker who said 20 seconds and 20 words or more in a meeting, in phrases rather than single words, and the next one knows them by voice.
        </p>
      ) : (
        <ul className="meetings-voices">
          {voices.map((v) => (
            <li key={v.id} className="meetings-voice">
              <span className="meetings-row-name">{v.name}</span>
              <span className="meetings-row-meta">
                {v.meetings} {v.meetings === 1 ? "meeting" : "meetings"}
              </span>
              <button type="button" className="meetings-row-delete" title="Forget" aria-label={`Forget ${v.name}`} onClick={() => setForgetting(v)}>
                <Trash2 size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <Modal
        title={`Forget ${forgetting?.name ?? ""}'s voice?`}
        open={forgetting !== null}
        onClose={() => setForgetting(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setForgetting(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (forgetting) void remove(forgetting.id);
                setForgetting(null);
              }}
            >
              Forget
            </button>
          </>
        }
      >
        <p className="meetings-confirm">Every sample of it goes. The name stays where you gave it, as text.</p>
      </Modal>
    </PageSheet>
  );
}
