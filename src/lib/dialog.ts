import { open, save } from "@tauri-apps/plugin-dialog";

/**
 * The platform's folder picker.
 *
 * The one place the app does *not* draw its own dialog: a file chooser is the
 * system's, with the user's sidebar, their recent places and their idea of
 * where things live. Drawing our own would be a worse file browser wearing
 * the right colours.
 *
 * `null` when the user cancelled — and outside the app, where there is no
 * picker to open.
 */
export async function pickFolder(): Promise<string | null> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return null;
  const chosen = await open({ directory: true, multiple: false, title: "Open folder" });
  return typeof chosen === "string" ? chosen : null;
}

/**
 * The platform's save dialog, for a file leaving the app.
 *
 * `null` when the user cancelled — and outside the app, where there is no
 * dialog to open.
 */
export async function pickSavePath(defaultName: string, extension: string): Promise<string | null> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return null;
  const chosen = await save({
    defaultPath: safeName(defaultName, extension),
    filters: [{ name: extension.toUpperCase(), extensions: [extension] }],
  });
  return chosen ?? null;
}

/** A file name from text that was never meant to be one — a chat title, say. */
export function safeName(text: string, extension: string): string {
  const cleaned = text.replace(/[\\/:*?"<>|\u0000-\u001f]/g, " ").replace(/\s+/g, " ").trim();
  return `${(cleaned || "chat").slice(0, 60)}.${extension}`;
}

/**
 * The platform's file picker, for one file of any kind.
 *
 * `null` when the user cancelled — and outside the app, where there is no
 * picker to open.
 */
export async function pickFile(title: string): Promise<string | null> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return null;
  const chosen = await open({ directory: false, multiple: false, title });
  return typeof chosen === "string" ? chosen : null;
}
