import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./chat";

// The user's own shells, in the Terminal tab — `src-tauri/src/commands/terminal.rs`.

/** Mirrors `domain::terminal::TerminalState`. `code` is null when a signal ended the shell. */
export type TerminalState = { state: "running" } | { state: "exited"; code: number | null };

/** Mirrors `domain::terminal::TerminalInfo`. `shell` is what runs in it: `zsh`, `cmd`. */
export type TerminalInfo = { id: number; shell: string; state: TerminalState };

/**
 * The other half of `TERMINAL_EVENT` in `commands/terminal.rs`: one opened,
 * its shell ended, or it was closed. A signal to read the list again.
 */
export const TERMINAL_EVENT = "terminals:changed";

export async function onTerminalChanged(handler: (id: number) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ id: number }>(TERMINAL_EVENT, ({ payload }) => handler(payload.id));
}

/** Oldest first. */
export async function terminalList(): Promise<TerminalInfo[]> {
  if (!inTauri()) return [];
  return invoke<TerminalInfo[]>("terminal_list");
}

/** The user's login shell in the open folder. The screen resizes it once it has measured itself. */
export async function terminalOpen(cols = 80, rows = 24): Promise<TerminalInfo> {
  return invoke<TerminalInfo>("terminal_open", { cols, rows });
}

/**
 * What the terminal wrote so far, then everything it writes, as raw bytes —
 * until the returned function lets go. Only this channel is let go: a screen
 * drawn again has attached its own by then.
 */
export async function terminalAttach(id: number, onOutput: (bytes: Uint8Array) => void): Promise<() => void> {
  const channel = new Channel<ArrayBuffer>();
  channel.onmessage = (data) => onOutput(new Uint8Array(data));
  await invoke("terminal_attach", { id, onOutput: channel });
  return () => {
    channel.onmessage = () => {};
    // The terminal may be closed already; nothing is attached to it then.
    invoke("terminal_detach", { id, channel: channel.id }).catch(() => {});
  };
}

/** A shell that has ended refuses input; the screen has nothing to say about it. */
export async function terminalWrite(id: number, data: string): Promise<void> {
  return invoke<void>("terminal_write", { id, data }).catch(() => {});
}

export async function terminalResize(id: number, cols: number, rows: number): Promise<void> {
  return invoke<void>("terminal_resize", { id, cols, rows }).catch(() => {});
}

/** Hangs up on the shell, as closing a terminal window does. */
export async function terminalClose(id: number): Promise<void> {
  return invoke<void>("terminal_close", { id });
}

/**
 * A selection from a terminal, as it goes into the message: which terminal,
 * then the text in a fence longer than any run of backticks inside it.
 */
export function terminalQuote(terminal: TerminalInfo, selection: string): string {
  const longest = Math.max(0, ...(selection.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `From my terminal (${terminal.shell} #${terminal.id}):\n${fence}\n${selection.trimEnd()}\n${fence}`;
}

export function terminalTitle(terminal: TerminalInfo): string {
  if (terminal.state.state === "running") return terminal.shell;
  return terminal.state.code === null ? `${terminal.shell} — ended` : `${terminal.shell} — exited ${terminal.state.code}`;
}
