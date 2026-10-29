import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";

// Background processes the agent started, for the Processes tab and the chat.

/** Mirrors `domain::background::ProcessState`. */
export type ProcessState = { state: "running" } | { state: "exited"; code: number | null } | { state: "stopped" };
/** Mirrors `domain::background::ProcessInfo`. */
export type ProcessInfo = { id: number; command: string; cwd: string; state: ProcessState };
/** A row of the Terminal tab: the process and the end of what it wrote. */
export type ProcessView = ProcessInfo & { tail: string };

/**
 * A background process started, wrote, ended or was stopped. Pinned on the
 * Rust side in `commands/processes.rs`.
 */
export const PROCESS_EVENT = "processes:changed";

export async function onProcessChanged(handler: (id: number) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ id: number }>(PROCESS_EVENT, ({ payload }) => handler(payload.id));
}

/** The ones chat `chatId` started; with no chat, every one — a folder switch stops them all. */
export async function processesList(chatId: string | null = null): Promise<ProcessView[]> {
  if (!inTauri()) return [];
  return invoke<ProcessView[]>("processes_list", { chatId });
}

/** The running ones chat `chatId` started, newest first, without output — cheap to read on every change. */
export async function processesRunning(chatId: string): Promise<ProcessInfo[]> {
  if (!inTauri()) return [];
  return invoke<ProcessInfo[]>("processes_running", { chatId });
}

/** The model is told at its next round that the user stopped it. Answers with chat `chatId`'s list. */
export async function stopProcess(id: number, chatId: string): Promise<ProcessView[]> {
  requireBackend();
  return invoke<ProcessView[]>("process_stop", { id, chatId });
}

/** How a process stands, in a few words: `running`, `exit 1`, `stopped`. */
export function processStatus(state: ProcessState): string {
  switch (state.state) {
    case "running":
      return "running";
    case "stopped":
      return "stopped";
    case "exited":
      return state.code === null ? "killed by a signal" : `exit ${state.code}`;
  }
}
