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

/**
 * Every version the chat kept, oldest first: what the model presented, and
 * what a restore made — the version put back, and the folder as it was before
 * when that had not been kept.
 */
export function versionsOf(blocks: Block[]): Presented[] {
  const versions: Presented[] = [];
  for (const block of blocks) {
    const presented = presentedOf(block);
    // Presented again unchanged, a page is the version it already was: one
    // entry, named as it was last.
    const again = presented ? versions.findIndex((v) => v.version === presented.version) : -1;
    if (presented && again >= 0) versions[again] = presented;
    else if (presented) versions.push(presented);
    if (block.kind !== "restored") continue;
    const from = versions.find((v) => v.version === block.from);
    const path = from?.path ?? versions[versions.length - 1]?.path ?? "index.html";
    const last = versions[versions.length - 1]?.path ?? path;
    if (block.before !== null) versions.push({ version: block.before, path: last, title: `Before restoring version ${block.from}` });
    versions.push({ version: block.version, path, title: `Version ${block.from}, restored` });
  }
  return versions;
}

/**
 * What the model is told with the next message when the user restored the
 * folder since their last one — what it remembers of the files is out of
 * date. `null` when nothing was restored since.
 */
export function restoreNote(blocks: Block[]): string | null {
  const lastUser = blocks.map((b) => b.kind).lastIndexOf("user");
  const restored = blocks.slice(lastUser + 1).filter((b): b is Extract<Block, { kind: "restored" }> => b.kind === "restored");
  const last = restored[restored.length - 1];
  if (!last) return null;
  return `[The user restored the chat's folder to version ${last.from} (now kept as version ${last.version}). The files are not what you last saw: read them before you edit them.]`;
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

/** What restoring a version did: mirrors `domain::prototype::Restored`. */
export type Restored = { from: number; version: number; before: number | null };

/** Puts the chat's folder back to `version`; the result is a new version, and nothing is lost. */
export async function restorePrototype(chatId: string, version: number): Promise<Restored> {
  requireBackend();
  return invoke<Restored>("prototype_restore", { chatId, version });
}

/** Shows the chat's working folder in Finder or Explorer. */
export async function revealPrototype(chatId: string): Promise<void> {
  requireBackend();
  return invoke<void>("prototype_reveal", { chatId });
}

/**
 * Saves `page` — of the working folder (`version` null) or of a version — as
 * one HTML file at `path`, the libraries inside it: it opens in any browser,
 * from disk, with no network.
 */
export async function exportPrototype(chatId: string, version: number | null, page: string, path: string): Promise<void> {
  requireBackend();
  return invoke<void>("prototype_export", { chatId, version, page, path });
}

/** An element the user pointed at on a page in the preview, to ask for a change to it alone. */
export type Pointed = { page: string; selector: string; html: string; text: string };

/** The most of the element's HTML the model is sent: enough to find it, not the page. */
const POINTED_HTML = 800;

/**
 * What the preview's pointer said it picked on `page`, if that is what it is.
 * The page is the model's code, so it is checked like anything else it sends.
 */
export function pointedOf(data: unknown, page: string): Pointed | null {
  const d = data as { kibo?: unknown; selector?: unknown; html?: unknown; text?: unknown } | null;
  if (d?.kibo !== "picked" || typeof d.selector !== "string" || typeof d.html !== "string" || typeof d.text !== "string") return null;
  const selector = d.selector.trim().slice(0, 300);
  if (!selector) return null;
  const html = d.html.length > POINTED_HTML ? `${d.html.slice(0, POINTED_HTML)}…` : d.html;
  return { page, selector, html, text: d.text.trim().slice(0, 200) };
}

/** What the model is told with the message, after what the user typed, about the element it is about. */
export function pointNote(pointed: Pointed): string {
  return [
    `[The user pointed at an element on the page ${pointed.page}: \`${pointed.selector}\`. As the browser drew it:`,
    "```html",
    pointed.html,
    "```",
    "Their message is about this element: change it, and only what they ask, with editFile — do not write the page again. " +
      "On a React page this is the rendered HTML: find the element in the source by its text and classes.]",
  ].join("\n");
}
