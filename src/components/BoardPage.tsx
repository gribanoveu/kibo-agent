import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { DragDropProvider, KeyboardSensor, PointerSensor, useDroppable, type DragDropEventHandlers } from "@dnd-kit/react";
import { useSortable } from "@dnd-kit/react/sortable";
import { move } from "@dnd-kit/helpers";
import { Archive, CalendarDays, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, MessageSquare, PanelRightOpen, Plus, Trash2 } from "lucide-react";
import { Page } from "./Page";
import { BoardCardSheet } from "./BoardCardSheet";
import { useBoard } from "../hooks/useBoard";
import { InfoTip } from "./InfoTip";
import { useFeatures } from "../hooks/useFeatures";
import { DONE_KEEP, dropChange, dueState, filterOrder, hasAnyTag, localDay, orderOf, tagsOf, type BoardChange, type Card, type Column, type OpenColumn, type Order } from "../lib/board";
import { matches, SHORTCUTS, type ShortcutId } from "../lib/shortcuts";
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
    in any column, opened in a sheet to edit, and any can be removed. Done keeps
    the last five finished — the backend sends older ones to the archive, shown
    under the columns on request. */
export function BoardPage() {
  const { cards, error, change } = useBoard();
  const dictate = useFeatures().isOn("voice-input");
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const [openId, setOpenId] = useState<string | null>(null);
  // The tags picked above the columns: a card shows when it has any of them.
  // One no card has any longer is let go of.
  const [picked, setPicked] = useState<string[]>([]);
  const tags = tagsOf(cards);
  const filter = picked.filter((tag) => tags.includes(tag));
  const passes = (card: Card) => filter.length === 0 || hasAnyTag(card, filter);
  // A click that ends a drag is not one that opens the card.
  const draggingRef = useRef(false);
  // While a card is dragged, the order it is dragged through; the board's
  // own once the drop is kept.
  const [dragged, setDragged] = useState<Order | null>(null);
  const draggedRef = useRef(dragged);
  draggedRef.current = dragged;
  const byId = new Map(cards.map((card) => [card.id, card]));
  const shownOrder = () => filterOrder(orderOf(cards), (id) => passes(byId.get(id)!));
  const order = dragged ?? shownOrder();
  // Newest first: the one just archived is the one looked for.
  const archived = cards.filter((card) => card.column === "archived" && passes(card)).reverse();
  const open = cards.filter((card) => card.column !== "archived").length;
  // Looked up on every read: a card removed, here or by the model, closes its sheet.
  const shown = openId === null ? undefined : byId.get(openId);
  const openCard = (id: string) => !draggingRef.current && setOpenId(id);

  const onDragEnd: DragDropEventHandlers["onDragEnd"] = (event) => {
    // The click the drop makes comes in the same task, before this runs.
    setTimeout(() => (draggingRef.current = false));
    const id = String(event.operation.source?.id);
    const next = !event.canceled && draggedRef.current && dropChange(shownOrder(), draggedRef.current, id, orderOf(cards));
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
    <Page
      title="Board"
      bar={tags.length > 0 && <TagFilter tags={tags} picked={filter} onPick={setPicked} />}
      sheet={shown && <BoardCardSheet key={shown.id} card={shown} tags={tags} dictate={dictate} error={error} onChange={change} onClose={() => setOpenId(null)} />}
    >
      <DragDropProvider
        sensors={SENSORS}
        onDragStart={() => {
          draggingRef.current = true;
          setDragged(shownOrder());
        }}
        onDragOver={(event) => setDragged((order) => order && move(order, event))}
        onDragEnd={onDragEnd}
      >
      {error && !shown && <p className="board-error">{error}</p>}
      {open === 0 && !error && (
        <p className="board-empty">Nothing here yet. Add a card, or ask the Assistant to remind you of something, and it lands here.</p>
      )}
      <div className="board-columns">
        {COLUMNS.map(({ column, label }, at) => {
          const here = order[column].flatMap((id) => byId.get(id) ?? []);
          return (
            <BoardColumn
              key={column}
              column={column}
              label={label}
              count={here.length}
              empty={column === "todo" && adding ? null : filter.length ? "No cards with these tags" : EMPTY[column]}
              action={
                column === "todo" && (
                  <button type="button" className="board-add" title="Add a card" aria-label="Add a card" disabled={adding} onClick={() => setAdding(true)}>
                    <Plus size={14} />
                  </button>
                )
              }
            >
              {column === "todo" && adding && (
                <CardEditor
                  tags={filter}
                  onSave={(title, note, due) => change({ op: "add", title, note, due, ...(filter.length > 0 && { tags: filter }) })}
                  onClose={() => setAdding(false)}
                />
              )}
              {here.map((card, index) => (
                <BoardCard
                  key={card.id}
                  card={card}
                  index={index}
                  group={column}
                  back={COLUMNS[at - 1]}
                  forward={COLUMNS[at + 1]}
                  selected={card.id === openId}
                  onOpen={openCard}
                  onChange={change}
                />
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
            {archiveOpen ? <ChevronUp className="board-archive-chev" size={14} /> : <ChevronDown className="board-archive-chev" size={14} />}
          </button>
          {archiveOpen && (
            <div className="board-archive-list">
              {archived.map((card, index) => (
                <BoardCard key={card.id} card={card} index={index} selected={card.id === openId} onOpen={openCard} onChange={change} />
              ))}
            </div>
          )}
        </section>
      )}
      </DragDropProvider>
    </Page>
  );
}

/** The board's tags as toggles over the columns, and All, which lets go of
    them; with some picked, the cards that have any of them are shown. */
function TagFilter({ tags, picked, onPick }: { tags: string[]; picked: string[]; onPick: (picked: string[]) => void }) {
  return (
    <div className="board-filter" role="group" aria-label="Show cards tagged">
      <button type="button" aria-pressed={picked.length === 0} onClick={() => onPick([])}>
        All
      </button>
      {tags.map((tag) => (
        <button
          key={tag}
          type="button"
          aria-pressed={picked.includes(tag)}
          onClick={() => onPick(picked.includes(tag) ? picked.filter((t) => t !== tag) : [...picked, tag])}
        >
          {tag}
        </button>
      ))}
    </div>
  );
}

/** What an empty column is for. */
const EMPTY: Record<OpenColumn, string> = {
  todo: "Add a card with + above",
  doing: "Drag a card here when you start on it",
  done: "Finished cards land here",
};

type ColumnProps = {
  column: OpenColumn;
  label: string;
  count: number;
  /** Said while it has no cards; none while a new one is being written in it. */
  empty: string | null;
  /** A button at the right end of its heading. */
  action?: ReactNode;
  children: ReactNode;
};

/** A column, and where a card is dropped when it has no card under it to go by. */
function BoardColumn({ column, label, count, empty, action, children }: ColumnProps) {
  // Low, so a card under the pointer wins over the column around it.
  const { ref } = useDroppable({ id: column, type: "column", accept: "card", collisionPriority: 1 });
  return (
    <section ref={ref} className="board-column" data-column={column} aria-label={label}>
      <h2 className="board-column-head">
        <span className="board-column-dot" aria-hidden="true" />
        {label}
        <span className="board-count">{count}</span>
        {action}
        {column === "done" && (
          <span className="board-column-tip">
            <InfoTip below text={`Done keeps the last ${DONE_KEEP} finished cards; older ones move to the Archive.`} />
          </span>
        )}
      </h2>
      {/* The cards scroll in their own box under the head, so the head is
          never over them and nothing shows through it. */}
      <div className="board-column-cards">
        {children}
        {count === 0 && empty && <p className="board-column-empty">{empty}</p>}
      </div>
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
  /** Whether its sheet is the one open, and what opens it: a click on the card, or its Open button. */
  selected: boolean;
  onOpen: (id: string) => void;
  /** Whether the board kept it. */
  onChange: (change: BoardChange) => Promise<boolean>;
};

function BoardCard({ card, index, group, back, forward, selected, onOpen, onChange }: CardProps) {
  const { ref } = useSortable({ id: card.id, index, group, type: "card", accept: "card", disabled: !group });
  const moveTo = (to: { column: Column; label: string }, Icon: typeof ChevronLeft) => (
    <button type="button" title={`Move to ${to.label}`} aria-label={`Move to ${to.label}`} onClick={() => void onChange({ op: "move", id: card.id, column: to.column })}>
      <Icon size={14} />
    </button>
  );
  return (
    // Its buttons do their own thing; a click anywhere else opens it.
    <article
      ref={ref}
      className={selected ? "board-card selected" : "board-card"}
      data-card-id={card.id}
      data-sheet-opener
      onClick={(e) => !(e.target instanceof Element && e.target.closest("button")) && onOpen(card.id)}
    >
      <div className="board-card-title">{card.title}</div>
      {card.note && <Note note={card.note} />}
      {(card.tags?.length ?? 0) > 0 && (
        <div className="board-card-tags">
          {card.tags!.map((tag) => (
            <span key={tag} className="board-card-tag">
              {tag}
            </span>
          ))}
        </div>
      )}
      <div className="board-card-foot">
        <span className="board-card-meta">
          <span className="board-card-id">{card.id}</span>
          {card.due && <Due due={card.due} open={card.column === "todo" || card.column === "doing"} />}
          {(card.comments?.length ?? 0) > 0 && (
            <span className="board-card-comments" title={`${card.comments!.length} comments`}>
              <MessageSquare size={12} />
              {card.comments!.length}
            </span>
          )}
        </span>
        <span className="board-card-tools">
          {back && moveTo(back, ChevronLeft)}
          {forward && moveTo(forward, ChevronRight)}
          <button type="button" title="Open" aria-label="Open" onClick={() => onOpen(card.id)}>
            <PanelRightOpen size={14} />
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
  /** The tags it will get — the filter's — shown so the card is not a surprise. */
  tags: string[];
  onSave: (title: string, note: string, due: string) => Promise<boolean>;
  onClose: () => void;
};

/** A new card's title, note and due day being written. Enter in the title or ⌘Enter in
    the note saves, Escape leaves it as it was; a refused one stays open, with
    the reason over the columns. */
function CardEditor({ tags, onSave, onClose }: EditorProps) {
  const [title, setTitle] = useState("");
  const [note, setNote] = useState("");
  const [due, setDue] = useState("");
  const save = async () => {
    if (title.trim() && (await onSave(title, note, due))) onClose();
  };
  return (
    <div
      className="board-card board-editor"
      role="group"
      aria-label="New card"
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
      {tags.length > 0 && (
        <div className="board-card-tags" aria-label="Tags it gets">
          {tags.map((tag) => (
            <span key={tag} className="board-card-tag">
              {tag}
            </span>
          ))}
        </div>
      )}
      <div className="board-editor-actions">
        <label className="board-editor-due">
          Due
          <input type="date" value={due} onChange={(e) => setDue(e.currentTarget.value)} />
        </label>
        <button type="button" className="btn btn-ghost" onClick={onClose}>
          Cancel
        </button>
        <button type="button" className="btn btn-primary" disabled={!title.trim()} onClick={() => void save()}>
          Add
        </button>
      </div>
    </div>
  );
}
