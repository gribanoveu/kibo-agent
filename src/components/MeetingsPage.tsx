import { useRef, useState } from "react";
import { matches } from "../lib/shortcuts";
import { AudioLines, Calendar, ShieldAlert, Check, ChevronLeft, Clock, Copy, FileAudio, FileText, Hourglass, LoaderCircle, MessageSquareText, Mic, MicOff, Pause, Play, Plus, RefreshCw, Sparkles, Square, Trash2, Users, X } from "lucide-react";
import { Page, PagePanel } from "./Page";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { Switch } from "./Switch";
import { useMeetings, type Job } from "../hooks/useMeetings";
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
  type Meeting,
  type MeetingInfo,
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
export function MeetingsPage({ sideHandle }: PageContext) {
  const meetings = useMeetings();
  const { job, meeting, openId } = meetings;
  // A meeting, or a recording not yet transcribed — the only copy of its call.
  const [deleting, setDeleting] = useState<{ id: string; title: string; recording: boolean } | null>(null);
  const remembering = useFeatures().isOn("voice-memory");
  const [voicesOpen, setVoicesOpen] = useState(false);
  const player = usePlayer(openId && audioUrl(openId));
  const access = useRecordingAccess();

  const bar = (
    <div className="meetings-bar">
      {openId && (
        <button type="button" className="btn btn-ghost" onClick={() => meetings.open(null)}>
          <ChevronLeft size={14} />
          All meetings
        </button>
      )}
      {!openId && (
        <button type="button" className="btn btn-primary" disabled={job !== null} onClick={() => void meetings.importRecording()}>
          <Plus size={14} />
          Import recording
        </button>
      )}
      {!openId && (
        <button
          type="button"
          className="btn btn-ghost"
          disabled={meetings.recording !== null || !access.ready}
          title={access.ready ? "Record a call: your microphone and what the computer plays" : "Allow the microphone and system audio first"}
          onClick={() => void meetings.record()}
        >
          <Mic size={14} />
          Record
        </button>
      )}
      {!openId && (
        <label className="meetings-auto" title="A recording is transcribed as soon as it is stopped">
          <Switch label="Auto-transcribe recordings" on={meetings.autoProcess} onChange={() => void meetings.toggleAutoProcess()} />
          Auto-transcribe recordings
        </label>
      )}
      {!openId && (
        <label className="meetings-auto" title="Your side of a recorded call is cleaned up before it is transcribed; the recording itself stays as it was">
          <Switch label="Reduce noise on my microphone" on={meetings.denoise} onChange={() => void meetings.toggleDenoise()} />
          Reduce noise on my microphone
        </label>
      )}
      {meeting && <PlayerBar player={player} seconds={meeting.seconds} />}
      {remembering && (
        <button type="button" className="btn btn-ghost meetings-voices-toggle" aria-pressed={voicesOpen} onClick={() => setVoicesOpen(!voicesOpen)}>
          <Users size={14} />
          Voices
        </button>
      )}
    </div>
  );

  // Voices change when a speaker is named (the meeting is read again) or a meeting is deleted (the list is).
  const panel = remembering && voicesOpen && <VoicesPanel changed={meeting ?? meetings.list} onClose={() => setVoicesOpen(false)} />;

  return (
    <Page title="Meetings" bar={bar} panel={panel} sideHandle={sideHandle}>
      {openId && <audio {...player.audio} />}
      {!openId && access.access && <AccessBanner access={access.access} onAllow={(source) => void access.allow(source)} />}
      {access.error && <p className="meetings-error">{access.error}</p>}
      {meetings.recording && (
        <RecorderCard recording={meetings.recording} onMute={(on) => void meetings.mute(on)} onStop={() => void meetings.stopRecording()} />
      )}
      {job && <JobCard job={job} title={titleOf(job, meetings.list)} onStop={meetings.stop} />}
      {meetings.error && <p className="meetings-error">{meetings.error}</p>}
      {openId === null ? (
        <MeetingList
          list={meetings.list}
          recordings={meetings.recordings}
          queued={meetings.queued}
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
            writing={job?.id === meeting.id && job.stage === "notes"}
            busy={job !== null}
            onSummarize={() => void meetings.summarize(meeting.id)}
            onRename={(speaker, name) => void meetings.rename(speaker, name)}
            player={player}
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
const SOURCE_NAMES: Record<Source, string> = { microphone: "Microphone — your side", call: "System audio — the call's side" };

/** What the system has not let the app record yet, each with the way to allow it: its
    prompt, or System Settings for one refused. Nothing when both are allowed or it does not say. */
function AccessBanner({ access, onAllow }: { access: RecordingAccess; onAllow: (source: Source) => void }) {
  const missing = SOURCES.filter((s) => access[s] === "denied" || access[s] === "notAsked");
  if (missing.length === 0) return null;
  return (
    <div className="meetings-access" role="note" aria-label="Recording permissions">
      <span className="meetings-access-icon" aria-hidden>
        <ShieldAlert size={16} />
      </span>
      <div className="meetings-access-body">
        <p className="meetings-access-title">To record a call, allow the app to hear:</p>
        {missing.map((source) => (
          <div key={source} className="meetings-access-row">
            <span className="meetings-access-name">{SOURCE_NAMES[source]}</span>
            <span className="meetings-access-state">{access[source] === "denied" ? "Turned off in System Settings" : "Not allowed yet"}</span>
            <button type="button" className="btn btn-ghost" onClick={() => onAllow(source)}>
              {access[source] === "denied" ? "Open System Settings" : "Allow"}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

/** What a recording not yet transcribed is called in the list. */
const recordedTitle = "Recorded call";

/** The call being recorded: how long, how loud each side is, your microphone's Mute, and Stop. */
function RecorderCard({ recording, onMute, onStop }: { recording: Recording; onMute: (on: boolean) => void; onStop: () => void }) {
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
        {callSilent(recording) && (
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
      <button type="button" className="btn btn-ghost" onClick={onStop}>
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
  const label = grouping ? "Grouping the voices" : stage ? STAGES[stage] : "Starting";
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
  onOpen,
  onDelete,
  onTranscribe,
  onDeleteRecording,
  job,
}: {
  list: MeetingInfo[];
  recordings: RecordingInfo[];
  queued: string[];
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

/** A call recorded and not yet transcribed: Transcribe makes it a meeting, or waits for the job running now. */
function RecordingRow({ r, queued, onTranscribe, onDelete }: { r: RecordingInfo; queued: boolean; onTranscribe: (id: string) => void; onDelete: (r: RecordingInfo) => void }) {
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
        {queued ? (
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
      <button type="button" className="meetings-row-delete" title="Delete" aria-label={`Delete ${recordedTitle} of ${time(r.created)}`} disabled={queued} onClick={() => onDelete(r)}>
        <Trash2 size={14} />
      </button>
    </li>
  );
}

function MeetingView({
  meeting,
  draft,
  writing,
  busy,
  onSummarize,
  onRename,
  player,
}: {
  meeting: Meeting;
  draft: string | null;
  writing: boolean;
  busy: boolean;
  onSummarize: () => void;
  onRename: (speaker: number, name: string) => void;
  player: Player;
}) {
  const { copied, copy } = useCopied();
  const notes = writing ? withoutTitle(draft ?? "") : meeting.summary;
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
          {meeting.title && (
            <span title="Imported from">
              <FileAudio size={13} />
              {meeting.name}
            </span>
          )}
        </div>
      </header>

      <section className="meeting-card" aria-label="Notes">
        <div className="meeting-card-head">
          <Sparkles size={14} className="meeting-card-icon" />
          <h3>Notes</h3>
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
        </div>
        <div className="meeting-notes">
          {notes ? (
            <Markdown text={notes} streaming={writing} />
          ) : (
            <p className="meetings-empty">{writing ? "Writing…" : "No notes yet — the model may have failed or been stopped."}</p>
          )}
        </div>
      </section>

      <section className="meeting-card" aria-label="Transcript">
        <div className="meeting-card-head">
          <MessageSquareText size={14} className="meeting-card-icon" />
          <h3>Transcript</h3>
        </div>
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
      </section>
    </article>
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
  const { voices, error, remove } = useVoices(changed);
  const [forgetting, setForgetting] = useState<VoiceInfo | null>(null);
  return (
    <PagePanel icon={Users} title="Voices" label="Voices" onClose={onClose}>
      {error && <p className="meetings-error">{error}</p>}
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
    </PagePanel>
  );
}
