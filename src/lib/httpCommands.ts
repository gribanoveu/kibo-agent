import { invoke } from "@tauri-apps/api/core";
import { requireBackend } from "./backend";

// The HTTP page's commands (docs/45-http-client.md). Mirrors `services::http_client`.

/** What came back: a response, 4xx and 5xx too, or why none did. */
export type HttpOutcome =
  | {
      kind: "response";
      status: number;
      headers: [string, string][];
      /** Where it came from, after any redirects. */
      url: string;
      /** The addresses redirected through, the first the request's own; absent when none. */
      redirects?: string[];
      elapsedMs: number;
    }
  | { kind: "failed"; error: string };

/** A request's response as the page shows it. `body` is null when it is not text; `size` is its bytes either way. */
export interface HttpResponseView {
  /** Unix milliseconds. */
  at: number;
  /** The environment's name it was sent into. */
  environment: string | null;
  outcome: HttpOutcome;
  body: string | null;
  size: number;
}

/**
 * Sends a collection's request. `request` is the page's unsaved Postman `request`
 * object, sent in place of the stored one. Rejects before anything is sent when a
 * variable is found nowhere; a stand that does not answer resolves as `failed`.
 */
export async function sendHttpRequest(
  collection: string,
  requestId: string,
  request: unknown,
  environment: string | null,
): Promise<HttpResponseView> {
  requireBackend();
  return invoke<HttpResponseView>("http_send", { collection, requestId, request, environment });
}

/** The request's last response, kept from its last send; null before the first. */
export async function lastHttpResponse(collection: string, requestId: string): Promise<HttpResponseView | null> {
  requireBackend();
  return invoke<HttpResponseView | null>("http_response_last", { collection, requestId });
}

/** Writes a Postman collection the page built at `dest`, a `.json` path from the save dialog. */
export async function exportHttpFile(dest: string, text: string): Promise<void> {
  requireBackend();
  return invoke("http_export", { dest, text });
}
