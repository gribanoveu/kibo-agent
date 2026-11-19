import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./backend";

/** The Assistant's board (`domain::board`): what the user asked to keep for later. */
/** `archived`: Done's oldest, past the last five (`domain::board::MAX_DONE`). */
export type Column = "todo" | "doing" | "done" | "archived";

export type Card = {
  id: string;
  title: string;
  note?: string;
  /** `YYYY-MM-DD`, a day in the user's own calendar. */
  due?: string;
  column: Column;
  /** The user's remarks on it, oldest first. */
  comments?: Comment[];
  /** The user's labels for it; the model is not shown them. */
  tags?: string[];
};

export type Comment = {
  /** Numbered within its card. */
  id: number;
  text: string;
  /** When it was written, RFC 3339. */
  at: string;
};

/** The page's own changes; the model adds and moves through its `board` tool.
    An `edit` replaces what it is given — the page gives all three; an empty
    note or due date clears it. */
export type BoardChange =
  /** `tags`: the page's filter, so a card added under it stays in sight. */
  | { op: "add"; title: string; note?: string; due?: string; tags?: string[] }
  | { op: "edit"; id: string; title?: string; note?: string; due?: string }
  /** `index`: its place among the column's other cards; Done ignores it. */
  | { op: "move"; id: string; column: Column; index?: number }
  | { op: "remove"; id: string }
  /** `at`: now, as `toISOString` writes it — the backend keeps no clock of its own. */
  | { op: "comment"; id: string; text: string; at: string }
  | { op: "uncomment"; id: string; comment: number }
  /** The whole list, replacing what the card had. */
  | { op: "tags"; id: string; tags: string[] };

export type OpenColumn = Exclude<Column, "archived">;
/** The open columns' card ids, top to bottom. */
export type Order = Record<OpenColumn, string[]>;

export const orderOf = (cards: Card[]): Order => ({
  todo: cards.filter((c) => c.column === "todo").map((c) => c.id),
  doing: cards.filter((c) => c.column === "doing").map((c) => c.id),
  done: cards.filter((c) => c.column === "done").map((c) => c.id),
});

const placeIn = (order: Order, id: string) => {
  const column = (Object.keys(order) as OpenColumn[]).find((c) => order[c].includes(id));
  return column && { column, index: order[column].indexOf(id) };
};

/** The move that puts card `id` where a drag left it, `before` → `after`; none
    when it is back where it started or was never there. `before` and `after`
    hold the cards shown — a tag filter may hide some of `all`, the board's
    whole order: the card then goes just above the shown card it was dropped
    above, or just below the one it was dropped under. */
export function dropChange(before: Order, after: Order, id: string, all: Order = before): BoardChange | null {
  const from = placeIn(before, id);
  const to = placeIn(after, id);
  if (!to || (from && from.column === to.column && from.index === to.index)) return null;
  const shown = after[to.column];
  const next = shown[to.index + 1];
  const prev = shown[to.index - 1];
  const rest = all[to.column].filter((other) => other !== id);
  const index = next !== undefined ? rest.indexOf(next) : prev !== undefined ? rest.indexOf(prev) + 1 : rest.length;
  return { op: "move", id, column: to.column, index };
}

/** The order with only the cards `keep` lets through. */
export const filterOrder = (order: Order, keep: (id: string) => boolean): Order => ({
  todo: order.todo.filter(keep),
  doing: order.doing.filter(keep),
  done: order.done.filter(keep),
});

/** Every tag on the board, each once whatever its case (the first spelling
    met), in alphabetical order. */
export const tagsOf = (cards: Card[]): string[] =>
  [...new Map(cards.flatMap((c) => c.tags ?? []).reverse().map((tag) => [tag.toLowerCase(), tag])).values()].sort((a, b) =>
    a.localeCompare(b, undefined, { sensitivity: "base" }),
  );

/** Whether a card has any of `picked`, in whatever case. */
export const hasAnyTag = (card: Card, picked: string[]): boolean =>
  (card.tags ?? []).some((tag) => picked.some((p) => p.toLowerCase() === tag.toLowerCase()));

/** A day as a card's `due` is written, in the user's own time zone — not
    `toISOString`, whose day is UTC's. */
export const localDay = (at = new Date()) =>
  `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;

/** Whether a due day has come; both `YYYY-MM-DD`, which compare as strings. */
export const dueState = (due: string, today: string): "past" | "today" | "ahead" =>
  due < today ? "past" : due === today ? "today" : "ahead";

export const BOARD_EVENT = "board:changed";

export async function onBoardChanged(handler: () => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen(BOARD_EVENT, () => handler());
}

export async function boardList(): Promise<Card[]> {
  if (!inTauri()) return [];
  return invoke<Card[]>("board_list");
}

/** The board after the change. */
export async function boardChange(change: BoardChange): Promise<Card[]> {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
  return invoke<Card[]>("board_change", { change });
}
