import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Copy, SquareKanban, Trash2, X } from "lucide-react";
import { PageSheet } from "./Page";
import { Dropdown } from "./Dropdown";
import { ChatMenu } from "./ChatMenu";
import { Markdown } from "./Markdown";
import { InfoTip } from "./InfoTip";
import { DictateButton } from "./DictateButton";
import { DictationLine } from "./VoiceWave";
import { ErrorNote } from "./ErrorNote";
import { useDictation } from "../hooks/useDictation";
import { dueState, localDay, type BoardChange, type Card, type Comment, type OpenColumn } from "../lib/board";
import { matches } from "../lib/shortcuts";
import "./BoardCardSheet.css";

const STATUS: { value: OpenColumn; label: string }[] = [
  { value: "todo", label: "To do" },
  { value: "doing", label: "Doing" },
  { value: "done", label: "Done" },
];

type Props = {
  card: Card;
  /** Every tag on the board, offered as the card's are written. */
  tags: string[];
  /** Voice input is on: a comment can be dictated, as a message is. */
  dictate: boolean;
  /** The board's refusal of the last change, shown here while the sheet is open. */
  error: string | null;
  onChange: (change: BoardChange) => Promise<boolean>;
  onClose: () => void;
};

/** One card, opened over the board's right side as a tracker's issue is: its id,
    status and actions on top, its title and description edited in place, then
    its fields one row each — a new field is one more `Field`. Each change is
    saved as it is made. */
export function BoardCardSheet({ card, tags, dictate, error, onChange, onClose }: Props) {
  const edit = (fields: Omit<Extract<BoardChange, { op: "edit" }>, "op" | "id">) => onChange({ op: "edit", id: card.id, ...fields });
  const status = STATUS.find((s) => s.value === card.column)?.label ?? "Archived";
  return (
    <PageSheet icon={SquareKanban} title="Card" label={`${card.id} details`} onClose={onClose}>
      <div className="board-sheet-top">
        <span className="board-sheet-key">{card.id}</span>
        <Dropdown
          label={<span className={`board-sheet-status ${card.column}`}>{status}</span>}
          title="Status"
          options={STATUS}
          value={card.column}
          below
          onPick={(column) => void onChange({ op: "move", id: card.id, column: column as OpenColumn })}
        />
        <span className="board-sheet-menu">
          <ChatMenu
            items={[
              { id: "copy", label: "Copy id", icon: <Copy size={14} />, onSelect: () => void navigator.clipboard.writeText(card.id).catch(() => {}) },
              { id: "remove", label: "Remove", icon: <Trash2 size={14} />, divided: true, onSelect: () => void onChange({ op: "remove", id: card.id }) },
            ]}
          />
        </span>
      </div>
      <Editable className="board-sheet-title" label="Title" value={card.title} required onSave={(title) => edit({ title })} />
      {error && <div className="board-sheet-error">{error}</div>}

      <h4 className="board-sheet-heading">Description</h4>
      <Editable
        className="board-sheet-note"
        label="Description"
        value={card.note ?? ""}
        multiline
        view={(note) => (note ? <Markdown text={note} streaming={false} /> : <span className="board-sheet-placeholder">Add a description…</span>)}
        onSave={(note) => edit({ note })}
      />

      <h4 className="board-sheet-heading">Details</h4>
      <dl className="board-sheet-fields">
        <Field
          label={
            <>
              Tags
              <InfoTip text="Yours alone: the Assistant does not see a card's tags." />
            </>
          }
        >
          <Tags tags={card.tags ?? []} known={tags} onChange={(tags) => onChange({ op: "tags", id: card.id, tags })} />
        </Field>
        <Field label="Due">
          <span className="board-sheet-due">
            <input type="date" aria-label="Due" value={card.due ?? ""} onChange={(e) => void edit({ due: e.currentTarget.value })} />
            {card.due && <DueFromNow due={card.due} open={card.column === "todo" || card.column === "doing"} />}
            {card.due && (
              <button type="button" title="Clear the due day" aria-label="Clear the due day" onClick={() => void edit({ due: "" })}>
                <X size={14} />
              </button>
            )}
          </span>
        </Field>
      </dl>

      <Comments
        card={card.id}
        dictate={dictate}
        comments={card.comments ?? []}
        onAdd={(text) => onChange({ op: "comment", id: card.id, text, at: new Date().toISOString() })}
        onRemove={(comment) => void onChange({ op: "uncomment", id: card.id, comment })}
      />
    </PageSheet>
  );
}

/** A row of the card's fields: its name, and what shows and changes it. */
function Field({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

/** What the tag box offers for `text`: the board's tags the card lacks that
    hold it, then — when the board has no such tag — creating it. `known`
    holds the card's own tags too. */
export function tagChoices(text: string, known: string[], has: string[]): { tag: string; create: boolean }[] {
  const typed = text.trim().toLowerCase();
  const lacks = known.filter((tag) => !has.some((h) => h.toLowerCase() === tag.toLowerCase()));
  const found = lacks.filter((tag) => tag.toLowerCase().includes(typed)).map((tag) => ({ tag, create: false }));
  const exists = known.some((tag) => tag.toLowerCase() === typed);
  return typed && !exists ? [...found, { tag: text.trim(), create: true }] : found;
}

/** The card's tags as chips, each with a button that takes it off, and a box
    that offers the board's other tags as it is typed in: ↑ ↓ pick one, Enter
    adds it (or creates what was typed), Escape closes the list. A tag no card
    has any longer leaves the board's list on its own. A refused one stays in
    the box. */
function Tags({ tags, known, onChange }: { tags: string[]; known: string[]; onChange: (tags: string[]) => Promise<boolean> }) {
  const [text, setText] = useState("");
  const [open, setOpen] = useState(false);
  // The highlighted choice; none until something is typed or an arrow is pressed.
  const [active, setActive] = useState(-1);
  const choices = tagChoices(text, known, tags);
  const listed = open && choices.length > 0;
  const add = async (tag: string) => {
    if (tag.trim() && (await onChange([...tags, tag]))) {
      setText("");
      setActive(-1);
    }
  };
  return (
    <span className="board-sheet-tags">
      {tags.map((tag) => (
        <span key={tag} className="board-sheet-tag">
          {tag}
          <button type="button" title={`Remove the tag ${tag}`} aria-label={`Remove the tag ${tag}`} onClick={() => void onChange(tags.filter((t) => t !== tag))}>
            <X size={12} />
          </button>
        </span>
      ))}
      <span className="board-sheet-tag-field">
        <input
          className="board-sheet-tag-box"
          role="combobox"
          aria-label="New tag"
          aria-autocomplete="list"
          aria-expanded={listed}
          aria-controls={listed ? "board-sheet-tag-list" : undefined}
          aria-activedescendant={listed && active >= 0 ? `board-sheet-tag-${active}` : undefined}
          autoComplete="off"
          placeholder={tags.length ? "Add" : "Add a tag"}
          value={text}
          onFocus={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onChange={(e) => {
            setText(e.currentTarget.value);
            setActive(e.currentTarget.value.trim() ? 0 : -1);
            setOpen(true);
          }}
          onKeyDown={(e) => {
            const moves = matches(e.nativeEvent, "tagSuggestNext") ? 1 : matches(e.nativeEvent, "tagSuggestPrev") ? -1 : 0;
            if (moves && listed) {
              e.preventDefault();
              setActive((active + moves + choices.length) % choices.length);
            } else if (matches(e.nativeEvent, "accept")) {
              e.preventDefault();
              void add(listed && active >= 0 ? choices[active].tag : text);
            } else if (matches(e.nativeEvent, "close") && listed) {
              setOpen(false);
            }
          }}
        />
        {listed && (
          <span id="board-sheet-tag-list" className="board-sheet-tag-list" role="listbox" aria-label="Tags">
            {choices.map((choice, at) => (
              <span
                key={`${choice.create}-${choice.tag}`}
                id={`board-sheet-tag-${at}`}
                role="option"
                aria-selected={at === active}
                className={at === active ? "board-sheet-tag-choice active" : "board-sheet-tag-choice"}
                // Keeps the focus in the box, so the list stays for the next one.
                onPointerDown={(e) => e.preventDefault()}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => void add(choice.tag)}
              >
                {choice.create ? `Create “${choice.tag}”` : choice.tag}
              </span>
            ))}
          </span>
        )}
      </span>
    </span>
  );
}

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** How far off the due day is, in words — "tomorrow", "3 days ago" — marked
    as the card marks it while the card is still open. */
function DueFromNow({ due, open }: { due: string; open: boolean }) {
  const today = localDay();
  const days = Math.round((new Date(`${due}T00:00`).getTime() - new Date(`${today}T00:00`).getTime()) / DAY);
  const text = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(days, "day");
  return <span className={`board-sheet-due-when ${open ? dueState(due, today) : "ahead"}`}>{text}</span>;
}

/** When a comment was written, as one says it: "5 minutes ago", "yesterday";
    past a week, the day itself. */
export function ago(at: string, now = Date.now()): string {
  const then = new Date(at).getTime();
  const past = now - then;
  const words = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  if (past < MINUTE) return words.format(0, "second");
  if (past < HOUR) return words.format(-Math.floor(past / MINUTE), "minute");
  if (past < DAY) return words.format(-Math.floor(past / HOUR), "hour");
  if (past < 7 * DAY) return words.format(-Math.floor(past / DAY), "day");
  return new Date(then).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

type CommentsProps = {
  /** Whose comments: a dictation into another card's box is not this one's. */
  card: string;
  dictate: boolean;
  comments: Comment[];
  /** Whether it was kept: the box is emptied only then. */
  onAdd: (text: string) => Promise<boolean>;
  onRemove: (comment: number) => void;
};

/** The user's remarks on the card: a box to write one — or dictate, with
    voice input on, by its microphone or the chat's hold key — then those there
    are, the newest first. ⌘Enter or Save adds it, Escape leaves the box. */
function Comments({ card, dictate, comments, onAdd, onRemove }: CommentsProps) {
  const [text, setText] = useState("");
  const [writing, setWriting] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  const dictation = useDictation({ on: dictate, draftKey: card, text, setText, area: box, onChange: () => {} });
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);
  const save = async () => {
    if (text.trim() && (await onAdd(text))) {
      setText("");
      setWriting(false);
    }
  };
  return (
    <section className="board-sheet-comments" aria-label="Comments">
      <h4 className="board-sheet-heading">
        Comments
        {comments.length > 0 && <span className="board-sheet-count">{comments.length}</span>}
      </h4>
      {dictation.error && <ErrorNote className="board-sheet-comment-error" message={dictation.error} onClose={dictation.clearError} />}
      <div className="board-sheet-comment-field">
      <textarea
        ref={box}
        className="board-sheet-comment-box"
        aria-label="New comment"
        placeholder="Add a comment…"
        rows={1}
        // What is said goes in as it is heard; typing waits for the dictation to end.
        readOnly={dictation.active}
        value={text}
        onFocus={() => setWriting(true)}
        onBlur={() => !text.trim() && setWriting(false)}
        onChange={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (matches(e.nativeEvent, "cardSave")) {
            e.preventDefault();
            void save();
          } else if (matches(e.nativeEvent, "close") && !text.trim()) {
            setWriting(false);
            e.currentTarget.blur();
          }
        }}
      />
      {dictate && <DictateButton dictation={dictation} />}
      </div>
      {dictation.active && <DictationLine peaks={dictation.peaks} partial={dictation.partial} />}
      {(writing || text || dictation.active) && (
        <div className="board-sheet-comment-actions">
          <button type="button" className="btn btn-ghost" disabled={dictation.active} onClick={() => (setText(""), setWriting(false))}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={!text.trim() || dictation.active} onClick={() => void save()}>
            Save
          </button>
        </div>
      )}
      <ol className="board-sheet-comment-list">
        {[...comments].reverse().map((comment) => (
          <li key={comment.id} className="board-sheet-comment">
            <div className="board-sheet-comment-head">
              <time dateTime={comment.at} title={new Date(comment.at).toLocaleString()}>
                {ago(comment.at)}
              </time>
              <button type="button" title="Delete the comment" aria-label="Delete the comment" onClick={() => onRemove(comment.id)}>
                <Trash2 size={13} />
              </button>
            </div>
            <div className="board-sheet-comment-text">
              <Markdown text={comment.text} streaming={false} />
            </div>
          </li>
        ))}
      </ol>
    </section>
  );
}

type EditableProps = {
  label: string;
  value: string;
  className: string;
  /** Lines of their own: Enter is a new line, ⌘Enter leaves it. */
  multiline?: boolean;
  /** An empty one goes back to what it was instead of being saved. */
  required?: boolean;
  /** What shows while it is not being written; a click on it, but on a link, starts writing. Without it, the field always shows. */
  view?: (value: string) => ReactNode;
  onSave: (value: string) => Promise<boolean>;
};

/** Text that reads as text until it is clicked into, as tall as what it holds.
    Leaving it saves; Enter (⌘Enter on its own lines) leaves it, Escape leaves it
    as it was. A refused one keeps what was typed, the reason above. */
function Editable({ label, value, className, multiline, required, view, onSave }: EditableProps) {
  const [draft, setDraft] = useState(value);
  const [writing, setWriting] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);
  // The model or the board may change it while it is shown.
  useEffect(() => setDraft(value), [value]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [draft, writing]);

  const commit = () => {
    setWriting(false);
    if (draft === value) return;
    if (required && !draft.trim()) setDraft(value);
    else void onSave(draft);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (matches(e.nativeEvent, "close")) {
      setDraft(value);
      // Left once the reset value is drawn, so blurring saves nothing.
      const field = e.currentTarget;
      requestAnimationFrame(() => field.blur());
    } else if (matches(e.nativeEvent, multiline ? "cardSave" : "accept")) {
      e.preventDefault();
      e.currentTarget.blur();
    }
  };

  if (view && !writing) {
    return (
      <div
        className={`${className} board-sheet-view`}
        role="button"
        tabIndex={0}
        aria-label={`Edit ${label.toLowerCase()}`}
        onClick={(e) => !(e.target instanceof Element && e.target.closest("a")) && setWriting(true)}
        onKeyDown={(e) => e.target === e.currentTarget && matches(e.nativeEvent, "accept") && (e.preventDefault(), setWriting(true))}
      >
        {view(value)}
      </div>
    );
  }
  return (
    <textarea
      ref={box}
      className={className}
      aria-label={label}
      rows={multiline ? 4 : 1}
      value={draft}
      autoFocus={writing}
      onFocus={() => setWriting(true)}
      onBlur={commit}
      onKeyDown={onKeyDown}
      onChange={(e) => setDraft(e.currentTarget.value)}
    />
  );
}
