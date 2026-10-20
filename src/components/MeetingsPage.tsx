import { useState } from "react";
import { matches } from "../lib/shortcuts";
import { ChevronLeft, Copy, Plus, RefreshCw, Square, Trash2, Users } from "lucide-react";
import { Page, PagePanel } from "./Page";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { useMeetings, type Job } from "../hooks/useMeetings";
import { useCopied } from "../hooks/useCopied";
import { useFeatures } from "../hooks/useFeatures";
import { useVoices } from "../hooks/useVoices";
import { meetingTitle, speakerName, withoutTitle, type Meeting, type MeetingInfo, type Stage, type VoiceInfo } from "../lib/meeting";
import type { PageContext } from "./pages";
import "./MeetingsPage.css";

const STAGES: Record<Stage, string> = {
  decode: "Reading the recording",
  speech: "Recognizing speech",
  speakers: "Telling the voices apart",
  notes: "Writing the notes",
};

/** `m:ss`, or `h:mm:ss` past an hour. */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const mm = String(Math.floor(s / 60) % 60).padStart(s >= 3600 ? 2 : 1, "0");
  const ss = String(s % 60).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${mm}:${ss}` : `${mm}:${ss}`;
}

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

  const bar = (
    <div className="meetings-bar">
      {openId && (
        <button type="button" className="btn btn-ghost" onClick={() => meetings.open(null)}>
          <ChevronLeft size={14} />
          All meetings
        </button>
      )}
      <button type="button" className="btn btn-primary" disabled={job !== null} onClick={() => void meetings.add()}>
        <Plus size={14} />
        Import recording
      </button>
      {job && <JobStatus job={job} onStop={meetings.stop} />}
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
      {meetings.error && <p className="meetings-error">{meetings.error}</p>}
      {openId === null ? (
        <MeetingList list={meetings.list} onOpen={meetings.open} onDelete={setDeleting} busy={job !== null} />
      ) : (
        meeting && (
          <MeetingView
            meeting={meeting}
            draft={meetings.draft}
            writing={job?.id === meeting.id && job.stage === "notes"}
            busy={job !== null}
            onSummarize={() => void meetings.summarize(meeting.id)}
            onRename={(speaker, name) => void meetings.rename(speaker, name)}
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

function JobStatus({ job, onStop }: { job: Job; onStop: () => void }) {
  const { stage, done = 0, total = 0 } = job;
  const share =
    stage === "speech" && total
      ? ` ${Math.round((100 * done) / total)}%`
      : stage === "notes" && done < total
        ? ` part ${done + 1} of ${total}`
        : "";
  return (
    <span className="meetings-job" role="status">
      <span className="meetings-job-text">
        {job.name && <b>{job.name}</b>} {job.stage ? `${STAGES[job.stage]}…${share}` : "Starting…"}
      </span>
      <button type="button" className="btn btn-ghost" onClick={onStop}>
        <Square size={12} />
        Stop
      </button>
    </span>
  );
}

function MeetingList({
  list,
  onOpen,
  onDelete,
  busy,
}: {
  list: MeetingInfo[];
  onOpen: (id: string) => void;
  onDelete: (m: MeetingInfo) => void;
  busy: boolean;
}) {
  if (list.length === 0) {
    return (
      <p className="meetings-empty">
        No meetings yet. Import a call's recording — sound or video — and its notes are written here: what was decided, who does what.
      </p>
    );
  }
  return (
    <ul className="meetings-list">
      {list.map((m) => (
        <li key={m.id} className="meetings-row">
          <button type="button" className="meetings-row-open" onClick={() => onOpen(m.id)}>
            <span className="meetings-row-name">{meetingTitle(m)}</span>
            <span className="meetings-row-meta">
              {when(m.created)} · {clock(m.seconds)} · {m.speakers} {m.speakers === 1 ? "speaker" : "speakers"}
              {!m.summarized && " · no notes"}
            </span>
          </button>
          <button type="button" className="meetings-row-delete" title="Delete" aria-label={`Delete ${meetingTitle(m)}`} disabled={busy} onClick={() => onDelete(m)}>
            <Trash2 size={14} />
          </button>
        </li>
      ))}
    </ul>
  );
}

function MeetingView({
  meeting,
  draft,
  writing,
  busy,
  onSummarize,
  onRename,
}: {
  meeting: Meeting;
  draft: string | null;
  writing: boolean;
  busy: boolean;
  onSummarize: () => void;
  onRename: (speaker: number, name: string) => void;
}) {
  const { copied, copy } = useCopied();
  const notes = writing ? withoutTitle(draft ?? "") : meeting.summary;
  return (
    <article className="meeting" aria-label={meetingTitle(meeting)}>
      <header className="meeting-head">
        <h2>{meetingTitle(meeting)}</h2>
        <span className="meetings-row-meta">
          {meeting.title && `${meeting.name} · `}
          {when(meeting.created)} · {clock(meeting.seconds)}
        </span>
      </header>

      <section className="meeting-notes" aria-label="Notes">
        <div className="meeting-section-head">
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
        {notes ? (
          <Markdown text={notes} streaming={writing} />
        ) : (
          <p className="meetings-empty">{writing ? "Writing…" : "No notes yet — the model may have failed or been stopped."}</p>
        )}
      </section>

      <section className="meeting-transcript" aria-label="Transcript">
        <h3>Transcript</h3>
        <Speakers meeting={meeting} onRename={onRename} />
        {meeting.utterances.map((u, i) => (
          <p key={i} className="meeting-line">
            <span className="meeting-time">{clock(u.start)}</span>
            <span className={`meeting-speaker speaker-${u.speaker % 4}`}>{speakerName(meeting, u.speaker)}</span>
            <span className="meeting-text">{u.text}</span>
          </p>
        ))}
      </section>
    </article>
  );
}

/** Who spoke, each renamed in place: the name goes into the transcript and the
    notes. Two given one name are one person the recognition split in two. */
function Speakers({ meeting, onRename }: { meeting: Meeting; onRename: (speaker: number, name: string) => void }) {
  const [editing, setEditing] = useState<number | null>(null);
  const count = Math.max(0, ...meeting.utterances.map((u) => u.speaker + 1));
  return (
    <div className="meeting-speakers" aria-label="Speakers">
      {Array.from({ length: count }, (_, speaker) =>
        editing === speaker ? (
          <input
            key={speaker}
            className="meeting-speaker-input"
            aria-label={`Name of speaker ${speaker + 1}`}
            placeholder={`Speaker ${speaker + 1}`}
            defaultValue={meeting.names?.[speaker] ?? ""}
            maxLength={60}
            autoFocus
            onKeyDown={(e) => {
              if (matches(e.nativeEvent, "accept")) {
                onRename(speaker, e.currentTarget.value);
                setEditing(null);
              } else if (matches(e.nativeEvent, "close")) {
                setEditing(null);
              }
            }}
            onBlur={() => setEditing(null)}
          />
        ) : (
          <button
            key={speaker}
            type="button"
            className={`meeting-speaker-chip speaker-${speaker % 4}${meeting.guessed?.includes(speaker) ? " guessed" : ""}`}
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
