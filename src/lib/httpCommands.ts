import { invoke } from "@tauri-apps/api/core";
import { requireBackend } from "./backend";

// The HTTP page's commands (docs/45-http-client.md). One until HC-2 keeps the collections.

/** Writes a Postman collection the page built at `dest`, a `.json` path from the save dialog. */
export async function exportHttpFile(dest: string, text: string): Promise<void> {
  requireBackend();
  return invoke("http_export", { dest, text });
}
