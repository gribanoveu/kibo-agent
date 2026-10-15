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

/** The page's own changes; the model adds and moves through its `board` tool. */
export type BoardChange = { op: "move"; id: string; column: Column } | { op: "remove"; id: string };

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
