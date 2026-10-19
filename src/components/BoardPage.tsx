import { useState } from "react";
import { Archive, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, Trash2 } from "lucide-react";
import { Page } from "./Page";
import { useBoard } from "../hooks/useBoard";
import type { BoardChange, Card, Column } from "../lib/board";
import type { PageContext } from "./pages";
import "./BoardPage.css";

const COLUMNS: { column: Column; label: string }[] = [
  { column: "todo", label: "To do" },
  { column: "doing", label: "Doing" },
  { column: "done", label: "Done" },
];

/** What the Assistant was asked to keep for later, in three columns. The model
    adds and moves cards; here they move too, and any can be removed. Done keeps
    the last five finished — the backend sends older ones to the archive, shown
    under the columns on request. */
export function BoardPage({ sideHandle }: PageContext) {
  const { cards, error, change } = useBoard();
  const [archiveOpen, setArchiveOpen] = useState(false);
  // Newest first: the one just archived is the one looked for.
  const archived = cards.filter((card) => card.column === "archived").reverse();
  const open = cards.length - archived.length;

  return (
    <Page title="Board" sideHandle={sideHandle}>
      {error && <p className="board-error">{error}</p>}
      {open === 0 && !error && (
        <p className="board-empty">Nothing here yet. Ask the Assistant to remind you of something, and it lands here.</p>
      )}
      <div className="board-columns">
        {COLUMNS.map(({ column, label }, at) => {
          const here = cards.filter((card) => card.column === column);
          return (
            <section key={column} className="board-column" aria-label={label}>
              <h2 className="board-column-head">
                {label}
                <span className="board-count">{here.length}</span>
              </h2>
              {here.map((card) => (
                <BoardCard
                  key={card.id}
                  card={card}
                  back={COLUMNS[at - 1]}
                  forward={COLUMNS[at + 1]}
                  onChange={(next) => void change(next)}
                />
              ))}
            </section>
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
              {archived.map((card) => (
                <BoardCard key={card.id} card={card} onChange={(next) => void change(next)} />
              ))}
            </div>
          )}
        </section>
      )}
    </Page>
  );
}

type CardProps = {
  card: Card;
  /** The columns either side, when there are. */
  back?: { column: Column; label: string };
  forward?: { column: Column; label: string };
  onChange: (change: BoardChange) => void;
};

function BoardCard({ card, back, forward, onChange }: CardProps) {
  const moveTo = (to: { column: Column; label: string }, Icon: typeof ChevronLeft) => (
    <button type="button" title={`Move to ${to.label}`} aria-label={`Move to ${to.label}`} onClick={() => onChange({ op: "move", id: card.id, column: to.column })}>
      <Icon size={14} />
    </button>
  );
  return (
    <article className="board-card">
      <div className="board-card-title">{card.title}</div>
      {card.note && <div className="board-card-note">{card.note}</div>}
      <div className="board-card-actions">
        <span className="board-card-id">{card.id}</span>
        {back && moveTo(back, ChevronLeft)}
        {forward && moveTo(forward, ChevronRight)}
        <button type="button" title="Remove" aria-label="Remove" onClick={() => onChange({ op: "remove", id: card.id })}>
          <Trash2 size={14} />
        </button>
      </div>
    </article>
  );
}
