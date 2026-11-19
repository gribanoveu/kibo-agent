import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Copy, SquareKanban, Trash2, X } from "lucide-react";
import { PageSheet } from "./Page";
import { Dropdown } from "./Dropdown";
import { ChatMenu } from "./ChatMenu";
import { Markdown } from "./Markdown";
import { dueState, localDay, type BoardChange, type Card, type OpenColumn } from "../lib/board";
import { matches } from "../lib/shortcuts";
import "./BoardCardSheet.css";

const STATUS: { value: OpenColumn; label: string }[] = [
  { value: "todo", label: "To do" },
  { value: "doing", label: "Doing" },
  { value: "done", label: "Done" },
];

type Props = {
  card: Card;
  /** The board's refusal of the last change, shown here while the sheet is open. */
  error: string | null;
  onChange: (change: BoardChange) => Promise<boolean>;
  onClose: () => void;
};

/** One card, opened over the board's right side as a tracker's issue is: its id,
    status and actions on top, its title and description edited in place, then
    its fields one row each — a new field is one more `Field`. Each change is
    saved as it is made. */
export function BoardCardSheet({ card, error, onChange, onClose }: Props) {
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
    </PageSheet>
  );
}

/** A row of the card's fields: its name, and what shows and changes it. */
function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

const DAY = 24 * 60 * 60 * 1000;

/** How far off the due day is, in words — "tomorrow", "3 days ago" — marked
    as the card marks it while the card is still open. */
function DueFromNow({ due, open }: { due: string; open: boolean }) {
  const today = localDay();
  const days = Math.round((new Date(`${due}T00:00`).getTime() - new Date(`${today}T00:00`).getTime()) / DAY);
  const text = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(days, "day");
  return <span className={`board-sheet-due-when ${open ? dueState(due, today) : "ahead"}`}>{text}</span>;
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
