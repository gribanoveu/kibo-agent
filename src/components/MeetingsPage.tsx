import { useRef, useState } from "react";
import { matches } from "../lib/shortcuts";
import { AudioLines, Calendar, Check, ChevronLeft, Clock, Copy, FileAudio, FileText, LoaderCircle, MessageSquareText, Pause, Play, Plus, RefreshCw, Sparkles, Square, Trash2, Users, X } from "lucide-react";
import { Page, PagePanel } from "./Page";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { useMeetings, type Job } from "../hooks/useMeetings";
import { useCopied } from "../hooks/useCopied";
import { useFeatures } from "../hooks/useFeatures";
import { useVoices } from "../hooks/useVoices";
import { usePlayer, type Player } from "../hooks/usePlayer";
import { audioUrl, meetingTitle, speakerName, withoutTitle, type Meeting, type MeetingInfo, type Stage, type VoiceInfo } from "../lib/meeting";
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
  const [deleting, setDeleting] = useState<MeetingInfo | null>(null);
  const remembering = useFeatures().isOn("voice-memory");
  const [voicesOpen, setVoicesOpen] = useState(false);
  const player = usePlayer(openId && audioUrl(openId));

  const bar = (
    <div className="meetings-bar">
      {openId && (
        <button type="button" className="btn btn-ghost" onClick={() => meetings.open(null)}>
          <ChevronLeft size={14} />
          All meetings
        </button>
      )}
      {!openId && (
        <button type="button" className="btn btn-primary" disabled={job !== null} onClick={() => void meetings.add()}>
          <Plus size={14} />
          Import recording
        </button>
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

  const panel = remembering && voicesOpen && <VoicesPanel changed={meeting ?? job} onClose={() => setVoicesOpen(false)} />;

  return (
    <Page title="Meetings" bar={bar} panel={panel} sideHandle={sideHandle}>
      {openId && <audio {...player.audio} />}
      {job && <JobCard job={job} title={titleOf(job, meetings.list)} onStop={meetings.stop} />}
      {meetings.error && <p className="meetings-error">{meetings.error}</p>}
      {openId === null ? (
        <MeetingList list={meetings.list} onOpen={meetings.open} onDelete={setDeleting} job={job} />
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
        title={`Delete ${deleting ? meetingTitle(deleting) : ""}?`}
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
                if (deleting) void meetings.remove(deleting.id);
                setDeleting(null);
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p className="meetings-confirm">Its transcript, notes and the app's copy of the recording go. The file it was imported from stays where it is.</p>
      </Modal>
    </Page>
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
  const part = share(job);
  const grouping = stage === "speakers" && total > 0 && done >= total;
  const label = grouping ? "Grouping the voices" : stage ? STAGES[stage] : "Starting";
  const detail = stage === "notes" && part !== null ? ` part ${done + 1} of ${total}` : part !== null && !grouping ? ` ${Math.round(100 * part)}%` : "";
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
                className={`meetings-job-fill${i === at && (part === null || grouping) ? " busy" : ""}`}
                style={{ width: i < at ? "100%" : i === at && part !== null && !grouping ? `${100 * part}%` : i === at ? "100%" : "0%" }}
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
  onOpen,
  onDelete,
  job,
}: {
  list: MeetingInfo[];
  onOpen: (id: string) => void;
  onDelete: (m: MeetingInfo) => void;
  job: Job | null;
}) {
  const busy = job !== null;
  if (list.length === 0) {
    return (
      <p className="meetings-empty">
        No meetings yet. Import a call's recording — sound or video — and its notes are written here: what was decided, who does what.
      </p>
    );
  }
  // Newest first already: a day's rows are together.
  const days: [string, MeetingInfo[]][] = [];
  for (const m of list) {
    const label = day(m.created);
    const last = days[days.length - 1];
    if (last?.[0] === label) last[1].push(m);
    else days.push([label, [m]]);
  }
  return (
    <div className="meetings-days">
      {days.map(([label, rows]) => (
        <section key={label} className="meetings-day" aria-label={label}>
          <h3 className="meetings-day-head">{label}</h3>
          <ul className="meetings-list">
            {rows.map((m) => (
              <li key={m.id} className="meetings-row">
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
            ))}
          </ul>
        </section>
      ))}
    </div>
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
  const speakers = Math.max(0, ...meeting.utterances.map((u) => u.speaker + 1));
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
                  className={`meeting-avatar speaker-${turn.speaker % 6}`}
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
                    <span className={`meeting-speaker speaker-${turn.speaker % 6}`}>{speakerName(meeting, turn.speaker)}</span>
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
  const count = Math.max(0, ...meeting.utterances.map((u) => u.speaker + 1));
  return (
    <div className="meeting-speakers" aria-label="Speakers" title="Click a name to rename the speaker">
      {Array.from({ length: count }, (_, speaker) =>
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
            className={`meeting-speaker-chip speaker-${speaker % 6}${meeting.guessed?.includes(speaker) ? " guessed" : ""}`}
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
