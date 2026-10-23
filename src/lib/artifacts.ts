import { invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";
import { onAnyToolResult } from "./chat";
import { pickSavePath } from "./dialog";
import { audioUrl, onMeetingEvent } from "./meeting";
import { pageUrl } from "./prototype";
import { windowsFrame } from "./window";

// The Artifacts page (docs/38-artifacts.md): what the app made, as folders —
// a kind, its collections (a meeting, a prototype), their files. Mirrors
// `domain::artifacts`.

export type ArtifactKind = "meetings" | "prototypes";

export type Collection = {
  kind: ArtifactKind;
  /** The meeting's id, the chat's. */
  id: string;
  title: string;
  /** RFC 3339: when the meeting was held, when the prototype was last presented. */
  updated: string;
  /** The recording's size; the prototype's working folder's. */
  bytes: number;
  /** The prototype's page, relative to its folder. */
  page?: string;
};

/** What a file opens in, decided by the backend. */
export type FileView = "markdown" | "code" | "html" | "image" | "audio" | "other";

export type ArtifactFile = {
  /** Inside the collection, with `/`: `assets/logo.png`. */
  path: string;
  bytes: number;
  view: FileView;
};

/** Every collection, newest first; meetings while their feature is on. */
export async function artifactList(): Promise<Collection[]> {
  if (!inTauri()) return [];
  return invoke<Collection[]>("artifact_list");
}

/** Bytes still free on the disk the artifacts are kept on; `null` outside the app. */
export async function artifactFreeSpace(): Promise<number | null> {
  if (!inTauri()) return null;
  return invoke<number>("artifact_free_space");
}

/** A collection's files, folders first. */
export async function artifactFiles(kind: ArtifactKind, id: string): Promise<ArtifactFile[]> {
  requireBackend();
  return invoke<ArtifactFile[]>("artifact_files", { kind, id });
}

/** A file as text, for a `markdown` or `code` view. */
export async function artifactRead(kind: ArtifactKind, id: string, path: string): Promise<string> {
  requireBackend();
  return invoke<string>("artifact_read", { kind, id, path });
}

/** Collection `id` called `title` for good; a prototype's refused while its chat works. */
export async function artifactRename(kind: ArtifactKind, id: string, title: string): Promise<void> {
  requireBackend();
  return invoke<void>("artifact_rename", { kind, id, title });
}

/** The folder a collection is kept in, shown in Finder or Explorer. */
export async function artifactReveal(kind: ArtifactKind, id: string): Promise<void> {
  requireBackend();
  return invoke<void>("artifact_reveal", { kind, id });
}

/**
 * The file saved where the user chooses in the system's dialog — a
 * prototype's page as one HTML file any browser opens. The dialog offers
 * `name`, else the file's own. `null` when they cancelled, else where it went.
 */
export async function artifactSaveAs(kind: ArtifactKind, id: string, path: string, name?: string): Promise<string | null> {
  requireBackend();
  const file = path.split("/").pop() ?? path;
  const dot = file.lastIndexOf(".");
  // ponytail: a file with no extension is offered as `name.`; the save dialog wants one — none in a prototype so far.
  const [stem, extension] = dot > 0 ? [file.slice(0, dot), file.slice(dot + 1)] : [file, ""];
  const dest = await pickSavePath(name ?? stem, extension);
  if (!dest) return null;
  await invoke<void>("artifact_save_as", { kind, id, path, dest });
  return dest;
}

/**
 * Where a file loads from when it is not read as text: a prototype's working
 * folder on `artifact://`, a meeting's recording on `meeting://`. `null` for
 * what only reads as text — a meeting's notes are written, not served.
 */
export function artifactUrl(kind: ArtifactKind, id: string, path: string, windows = windowsFrame): string | null {
  if (kind === "prototypes") return pageUrl(id, null, path, windows);
  return path === "audio.wav" ? audioUrl(id, windows) : null;
}

/**
 * Said whenever the collections may have changed, by the signals that already
 * say so: a meeting transcribed or its job ended, and in any turn a page
 * presented. A signal to read again — the whole list, so not on every edit.
 */
export function onCollectionsChanged(handler: () => void): Promise<UnlistenFn> {
  return onSignals(handler, false);
}

/**
 * Said whenever an open collection's files may have changed: what
 * [`onCollectionsChanged`] says, and in any turn a tool call that changed files.
 */
export function onArtifactsChanged(handler: () => void): Promise<UnlistenFn> {
  return onSignals(handler, true);
}

async function onSignals(handler: () => void, edits: boolean): Promise<UnlistenFn> {
  const stops = await Promise.all([
    onMeetingEvent((event) => (event.kind === "transcribed" || event.kind === "ended") && handler()),
    onAnyToolResult((result) => {
      const presented = (result.result as { result?: string } | undefined)?.result === "presented";
      if (presented || (edits && (result.changes?.length ?? 0) > 0)) handler();
    }),
  ]);
  return () => stops.forEach((stop) => stop());
}
