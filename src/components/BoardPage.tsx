import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { DragDropProvider, KeyboardSensor, PointerSensor, useDroppable, type DragDropEventHandlers } from "@dnd-kit/react";
import { useSortable } from "@dnd-kit/react/sortable";
import { move } from "@dnd-kit/helpers";
import { Archive, CalendarDays, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Pencil, Plus, Trash2 } from "lucide-react";
import { Page } from "./Page";
import { useBoard } from "../hooks/useBoard";
import { dropChange, dueState, localDay, orderOf, type BoardChange, type Card, type Column, type OpenColumn, type Order } from "../lib/board";
import { matches, SHORTCUTS, type ShortcutId } from "../lib/shortcuts";
import type { PageContext } from "./pages";
import "./BoardPage.css";

const COLUMNS: { column: OpenColumn; label: string }[] = [
  { column: "todo", label: "To do" },
  { column: "doing", label: "Doing" },
  { column: "done", label: "Done" },
];

// Pointer and keyboard, not HTML5 drag and drop: the window's own drop handler,
// which takes files dropped on the chat, keeps that from working on Windows.
const codes = (id: ShortcutId) => SHORTCUTS[id].combos.map((c) => c.code);
const [up, down, left, right] = codes("cardDragMove").map((code) => [code]);
const SENSORS = [
  PointerSensor,
  KeyboardSensor.configure({ keyboardCodes: { start: codes("cardDrag"), end: codes("cardDrag"), cancel: codes("close"), up, down, left, right } }),
];

/** What the Assistant was asked to keep for later, in three columns. The model
    adds and moves cards; here they are added to To do too, dragged to a place
    in any column, edited, and any can be removed. Done keeps
    the last five finished — the backend sends older ones to the archive, shown
    under the columns on request. */
export function BoardPage({ sideHandle }: PageContext) {
  const { cards, error, change } = useBoard();
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  // While a card is dragged, the order it is dragged through; the board's
  // own once the drop is kept.
  const [dragged, setDragged] = useState<Order | null>(null);
  const draggedRef = useRef(dragged);
  draggedRef.current = dragged;
  const order = dragged ?? orderOf(cards);
  const byId = new Map(cards.map((card) => [card.id, card]));
  // Newest first: the one just archived is the one looked for.
  const archived = cards.filter((card) => card.column === "archived").reverse();
  const open = cards.length - archived.length;

  const onDragEnd: DragDropEventHandlers["onDragEnd"] = (event) => {
    const id = String(event.operation.source?.id);
    const next = !event.canceled && draggedRef.current && dropChange(orderOf(cards), draggedRef.current, id);
    if (!next) {
      setDragged(null);
      return;
    }
    // A card dropped from the keyboard keeps the focus, in whichever column it lands.
    const keyboard = event.operation.activatorEvent instanceof KeyboardEvent;
    void change(next).then(() => {
      setDragged(null);
      if (keyboard) requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-card-id="${id}"]`)?.focus());
    });
  };

  return (
    <Page title="Board" sideHandle={sideHandle}>
      <DragDropProvider
        sensors={SENSORS}
        onDragStart={() => setDragged(orderOf(cards))}
        onDragOver={(event) => setDragged((order) => order && move(order, event))}
        onDragEnd={onDragEnd}
      >
      {error && <p className="board-error">{error}</p>}
      {open === 0 && !error && (
        <p className="board-empty">Nothing here yet. Add a card, or ask the Assistant to remind you of something, and it lands here.</p>
      )}
      <div className="board-columns">
        {COLUMNS.map(({ column, label }, at) => {
          const here = order[column].flatMap((id) => byId.get(id) ?? []);
          return (
            <BoardColumn key={column} column={column} label={label} count={here.length}>
              {here.map((card, index) => (
                <BoardCard
                  key={card.id}
                  card={card}
                  index={index}
                  group={column}
                  back={COLUMNS[at - 1]}
                  forward={COLUMNS[at + 1]}
                  onChange={change}
                />
              ))}
              {column === "todo" &&
                (adding ? (
                  <CardEditor onSave={(title, note, due) => change({ op: "add", title, note, due })} onClose={() => setAdding(false)} />
                ) : (
                  <button type="button" className="board-add" onClick={() => setAdding(true)}>
                    <Plus size={14} />
                    Add a card
                  </button>
                ))}
            </BoardColumn>
          );
        })}
      </div>
      {archived.length > 0 && (
        <section className="board-archive" aria-label="Archive">
          <button type="button" className="board-archive-toggle" aria-expanded={archiveOpen} onClick={() => setArchiveOpen(!archiveOpen)}>
            <Archive size={14} />
            Archive
            <span className="board-count">{archived.length}</span>
            {archiveOpen ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
          </button>
          {archiveOpen && (
            <div className="board-archive-list">
              {archived.map((card, index) => (
                <BoardCard key={card.id} card={card} index={index} onChange={change} />
              ))}
            </div>
          )}
        </section>
      )}
      </DragDropProvider>
    </Page>
  );
}

/** What an empty column is for — To do has Add a card instead. */
const EMPTY: Record<Exclude<OpenColumn, "todo">, string> = {
  doing: "Drag a card here when you start on it",
  done: "Finished cards land here",
};

/** A column, and where a card is dropped when it has no card under it to go by. */
function BoardColumn({ column, label, count, children }: { column: OpenColumn; label: string; count: number; children: ReactNode }) {
  // Low, so a card under the pointer wins over the column around it.
  const { ref } = useDroppable({ id: column, type: "column", accept: "card", collisionPriority: 1 });
  return (
    <section ref={ref} className="board-column" data-column={column} aria-label={label}>
      <h2 className="board-column-head">
        <span className="board-column-dot" aria-hidden="true" />
        {label}
        <span className="board-count">{count}</span>
      </h2>
      {children}
      {count === 0 && column !== "todo" && <p className="board-column-empty">{EMPTY[column]}</p>}
    </section>
  );
}

type CardProps = {
  card: Card;
  /** Its place in its column, and the column it is dragged in; an archived card has none and stays put. */
  index: number;
  group?: OpenColumn;
  /** The columns either side, when there are. */
  back?: { column: Column; label: string };
  forward?: { column: Column; label: string };
  /** Whether the board kept it. */
  onChange: (change: BoardChange) => Promise<boolean>;
};

function BoardCard({ card, index, group, back, forward, onChange }: CardProps) {
  const [editing, setEditing] = useState(false);
  const { ref } = useSortable({ id: card.id, index, group, type: "card", accept: "card", disabled: editing || !group });
  const moveTo = (to: { column: Column; label: string }, Icon: typeof ChevronLeft) => (
    <button type="button" title={`Move to ${to.label}`} aria-label={`Move to ${to.label}`} onClick={() => void onChange({ op: "move", id: card.id, column: to.column })}>
      <Icon size={14} />
    </button>
  );
  if (editing) {
    return <CardEditor card={card} onSave={(title, note, due) => onChange({ op: "edit", id: card.id, title, note, due })} onClose={() => setEditing(false)} />;
  }
  return (
    <article ref={ref} className="board-card" data-card-id={card.id}>
      <div className="board-card-title">{card.title}</div>
      {card.note && <Note note={card.note} />}
      <div className="board-card-foot">
        <span className="board-card-meta">
          <span className="board-card-id">{card.id}</span>
          {card.due && <Due due={card.due} open={card.column === "todo" || card.column === "doing"} />}
        </span>
        <span className="board-card-tools">
          {back && moveTo(back, ChevronLeft)}
          {forward && moveTo(forward, ChevronRight)}
          <button type="button" title="Edit" aria-label="Edit" onClick={() => setEditing(true)}>
            <Pencil size={14} />
          </button>
          <button type="button" title="Remove" aria-label="Remove" onClick={() => void onChange({ op: "remove", id: card.id })}>
            <Trash2 size={14} />
          </button>
        </span>
      </div>
    </article>
  );
}

/** A card's note, cut to three lines; Show more, when it runs past them,
    opens the rest in place. */
function Note({ note }: { note: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [cut, setCut] = useState(false);
  const [open, setOpen] = useState(false);
  // Measured while cut, and again as the column's width changes.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || open) return;
    const measure = () => setCut(el.scrollHeight > el.clientHeight + 1);
    measure();
    const watch = new ResizeObserver(measure);
    watch.observe(el);
    return () => watch.disconnect();
  }, [note, open]);
  return (
    <>
      <div ref={ref} className={open ? "board-card-note open" : "board-card-note"}>
        {note}
      </div>
      {(cut || open) && (
        <button type="button" className="board-card-more" aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? "Show less" : "Show more"}
        </button>
      )}
    </>
  );
}

/** A card's due day, short; marked once it has come while the card is still open. */
function Due({ due, open }: { due: string; open: boolean }) {
  const day = new Date(`${due}T00:00`);
  const thisYear = day.getFullYear() === new Date().getFullYear();
  const text = day.toLocaleDateString(undefined, { day: "numeric", month: "short", year: thisYear ? undefined : "numeric" });
  const state = open ? dueState(due, localDay()) : "ahead";
  return (
    <span className={`board-card-due ${state}`} title={state === "past" ? `Overdue — was due ${text}` : `Due ${text}`}>
      <CalendarDays size={12} />
      {text}
    </span>
  );
}

type EditorProps = {
  /** The card being edited; none for a new one. */
  card?: Card;
  onSave: (title: string, note: string, due: string) => Promise<boolean>;
  onClose: () => void;
};

/** A card's title, note and due day being written. Enter in the title or ⌘Enter in
    the note saves, Escape leaves it as it was; a refused one stays open, with
    the reason over the columns. */
function CardEditor({ card, onSave, onClose }: EditorProps) {
  const [title, setTitle] = useState(card?.title ?? "");
  const [note, setNote] = useState(card?.note ?? "");
  const [due, setDue] = useState(card?.due ?? "");
  const save = async () => {
    if (title.trim() && (await onSave(title, note, due))) onClose();
  };
  return (
    <div
      className="board-card board-editor"
      role="group"
      aria-label={card ? `Edit ${card.id}` : "New card"}
      onKeyDown={(e) => matches(e.nativeEvent, "close") && onClose()}
    >
      <input
        className="board-editor-title"
        aria-label="Title"
        placeholder="What to do"
        value={title}
        autoFocus
        onChange={(e) => setTitle(e.currentTarget.value)}
        onKeyDown={(e) => matches(e.nativeEvent, "accept") && void save()}
      />
      <textarea
        className="board-editor-note"
        aria-label="Note"
        placeholder="Details, optional"
        rows={3}
        value={note}
        onChange={(e) => setNote(e.currentTarget.value)}
        onKeyDown={(e) => matches(e.nativeEvent, "cardSave") && void save()}
      />
      <div className="board-editor-actions">
        <label className="board-editor-due">
          Due
          <input type="date" value={due} onChange={(e) => setDue(e.currentTarget.value)} />
        </label>
        <button type="button" className="btn btn-ghost" onClick={onClose}>
          Cancel
        </button>
        <button type="button" className="btn btn-primary" disabled={!title.trim()} onClick={() => void save()}>
          {card ? "Save" : "Add"}
        </button>
      </div>
    </div>
  );
}
