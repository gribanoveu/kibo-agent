import { invoke } from "@tauri-apps/api/core";
import { requireBackend } from "./backend";
import type { Block } from "./chatTurnReducer";
import { windowsFrame } from "./window";

// A Prototype chat's pages (docs/31-prototype-role.md). What was presented is
// not state of its own: it is the chat's `present` calls the backend kept —
// their results say which page and which version — so the transcript, saved
// with the chat, is the list.

/** One `present` call that kept a version. */
export type Presented = { version: number; path: string; title: string };

/** The page a settled `present` call showed; `null` for any other block, or one refused. */
export function presentedOf(block: Block): Presented | null {
  if (block.kind !== "tool" || block.name !== "present" || block.status !== "done") return null;
  const result = block.result as { result?: string; version?: unknown; path?: unknown; title?: unknown } | undefined;
  if (result?.result !== "presented" || typeof result.version !== "number" || typeof result.path !== "string") return null;
  return { version: result.version, path: result.path, title: typeof result.title === "string" ? result.title : result.path };
}

/** Every version the chat presented, oldest first. */
export function versionsOf(blocks: Block[]): Presented[] {
  return blocks.flatMap((block) => presentedOf(block) ?? []);
}

/** Calls that change the working folder: each done one is a reason to draw it again. */
const CHANGES_FILES = new Set(["writeFile", "editFile", "delete", "move"]);

/** How many times the working folder has changed in the transcript — a key that moves with every edit. */
export function edits(blocks: Block[]): number {
  return blocks.filter((block) => block.kind === "tool" && block.status === "done" && CHANGES_FILES.has(block.name)).length;
}

/**
 * Where the preview loads `path` from: the working folder (`version` null) or
 * a presented version. The scheme's address is the platform's — Windows
 * serves an app's scheme as `http://<scheme>.localhost`.
 */
export function pageUrl(chatId: string, version: number | null, path: string, windows = windowsFrame): string {
  const root = windows ? "http://artifact.localhost" : "artifact://localhost";
  const where = version === null ? "files" : `versions/${version}`;
  return `${root}/${encodeURIComponent(chatId)}/${where}/${path.split("/").map(encodeURIComponent).join("/")}`;
}

/** Shows the chat's working folder in Finder or Explorer. */
export async function revealPrototype(chatId: string): Promise<void> {
  requireBackend();
  return invoke<void>("prototype_reveal", { chatId });
}
