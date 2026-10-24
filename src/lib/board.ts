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
  column: Column;
};

/** The page's own changes; the model adds and moves through its `board` tool.
    An `edit` replaces both the title and the note; an empty note clears it. */
export type BoardChange =
  | { op: "add"; title: string; note?: string }
  | { op: "edit"; id: string; title: string; note?: string }
  /** `index`: its place among the column's other cards; Done ignores it. */
  | { op: "move"; id: string; column: Column; index?: number }
  | { op: "remove"; id: string };

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
    when it is back where it started or was never there. */
export function dropChange(before: Order, after: Order, id: string): BoardChange | null {
  const from = placeIn(before, id);
  const to = placeIn(after, id);
  if (!to || (from && from.column === to.column && from.index === to.index)) return null;
  return { op: "move", id, column: to.column, index: to.index };
}

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
