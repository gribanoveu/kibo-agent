import { useState } from "react";
import { ChevronLeft, Copy, Plus, RefreshCw, Square, Trash2 } from "lucide-react";
import { Page } from "./Page";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { useMeetings, type Job } from "../hooks/useMeetings";
import { useCopied } from "../hooks/useCopied";
import type { Meeting, MeetingInfo } from "../lib/meeting";
import type { PageContext } from "./pages";
import "./MeetingsPage.css";

const STAGES: Record<NonNullable<Job["stage"]>, string> = {
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
        Add recording
      </button>
      {job && <JobStatus job={job} onStop={meetings.stop} />}
    </div>
  );

  return (
    <Page title="Meetings" bar={bar} sideHandle={sideHandle}>
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
          />
        )
      )}
      <Modal
        title={`Delete ${deleting?.name ?? ""}?`}
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
        <p className="meetings-confirm">Its transcript and notes go. The recording itself stays where it is.</p>
      </Modal>
    </Page>
  );
}

function JobStatus({ job, onStop }: { job: Job; onStop: () => void }) {
  const share = job.stage === "speech" && job.total ? ` ${Math.round((100 * (job.done ?? 0)) / job.total)}%` : "";
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
        No meetings yet. Add a call's recording — sound or video — and its notes are written here: what was decided, who does what.
      </p>
    );
  }
  return (
    <ul className="meetings-list">
      {list.map((m) => (
        <li key={m.id} className="meetings-row">
          <button type="button" className="meetings-row-open" onClick={() => onOpen(m.id)}>
            <span className="meetings-row-name">{m.name}</span>
            <span className="meetings-row-meta">
              {when(m.created)} · {clock(m.seconds)} · {m.speakers} {m.speakers === 1 ? "speaker" : "speakers"}
              {!m.summarized && " · no notes"}
            </span>
          </button>
          <button type="button" className="meetings-row-delete" title="Delete" aria-label={`Delete ${m.name}`} disabled={busy} onClick={() => onDelete(m)}>
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
}: {
  meeting: Meeting;
  draft: string | null;
  writing: boolean;
  busy: boolean;
  onSummarize: () => void;
}) {
  const { copied, copy } = useCopied();
  const notes = writing ? (draft ?? "") : meeting.summary;
  return (
    <article className="meeting" aria-label={meeting.name}>
      <header className="meeting-head">
        <h2>{meeting.name}</h2>
        <span className="meetings-row-meta">
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
        {meeting.utterances.map((u, i) => (
          <p key={i} className="meeting-line">
            <span className="meeting-time">{clock(u.start)}</span>
            <span className={`meeting-speaker speaker-${u.speaker % 4}`}>Speaker {u.speaker + 1}</span>
            <span className="meeting-text">{u.text}</span>
          </p>
        ))}
      </section>
    </article>
  );
}
