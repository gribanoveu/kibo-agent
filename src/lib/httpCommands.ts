import { invoke } from "@tauri-apps/api/core";
import { requireBackend } from "./backend";
import type { Environment, EnvVariable } from "./httpClient";

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

/** A collection's Postman file as stored, or why it does not read. */
export interface HttpCollectionView {
  id: string;
  file: unknown | null;
  error: string | null;
}

/** One send in the log: the request as it left, a secret masked, and what came back — a body cut at 256 KB. */
export interface HttpHistoryEntry {
  id: string;
  at: number;
  environment: string | null;
  /** The chat whose agent sent it; null when the page did. */
  agentChat: string | null;
  /** The address before its variables were filled in. */
  templateUrl: string;
  /** The collection's request it was, which the entry opens. */
  requestId?: string;
  request: { method: string; url: string; headers: [string, string][]; body: string | null };
  outcome: HttpOutcome;
  responseBody: string | null;
}

export type HttpImported = { kind: "collection"; id: string; name: string } | { kind: "environment"; id: string; name: string } | { kind: "globals"; count: number };

/**
 * Sends a collection's request. `request` is the page's unsaved Postman `request`
 * object, sent in place of the stored one. Rejects before anything is sent when a
 * variable is found nowhere; a stand that does not answer resolves as `failed`.
 */
export async function sendHttpRequest(collection: string, requestId: string, request: unknown, environment: string | null): Promise<HttpResponseView> {
  requireBackend();
  return invoke<HttpResponseView>("http_send", { collection, requestId, request, environment });
}

/** The request's last response, kept from its last send; null before the first. */
export async function lastHttpResponse(collection: string, requestId: string): Promise<HttpResponseView | null> {
  requireBackend();
  return invoke<HttpResponseView | null>("http_response_last", { collection, requestId });
}

/** Every collection; opening the page this way also drops the responses of requests gone. */
export async function listHttpCollections(): Promise<HttpCollectionView[]> {
  requireBackend();
  return invoke<HttpCollectionView[]>("http_collection_list");
}

export async function newHttpCollection(name: string): Promise<HttpCollectionView> {
  requireBackend();
  return invoke<HttpCollectionView>("http_collection_new", { name });
}

export async function deleteHttpCollection(id: string): Promise<void> {
  requireBackend();
  return invoke("http_collection_delete", { id });
}

/** A request's Postman `request` object written over the stored one; the rest of its item stays. */
export async function saveHttpRequest(collection: string, requestId: string, request: unknown): Promise<void> {
  requireBackend();
  return invoke("http_request_save", { collection, requestId, request });
}

/** A Postman item added last in the folder `parent`, or in the collection when it is null. */
export async function addHttpItem(collection: string, parent: string | null, item: unknown): Promise<void> {
  requireBackend();
  return invoke("http_item_add", { collection, parent, item });
}

/** `id` is a folder's or request's — or the collection's own, to rename it. */
export async function renameHttpItem(collection: string, id: string, name: string): Promise<void> {
  requireBackend();
  return invoke("http_item_rename", { collection, id, name });
}

export async function deleteHttpItem(collection: string, id: string): Promise<void> {
  requireBackend();
  return invoke("http_item_delete", { collection, id });
}

/** A Postman collection, environment or globals file, by its path from the open dialog. */
export async function importHttpFile(path: string): Promise<HttpImported> {
  requireBackend();
  return invoke<HttpImported>("http_import", { path });
}

/** The stored collection at `dest`, a `.json` path from the save dialog. */
export async function exportHttpCollection(id: string, dest: string): Promise<void> {
  requireBackend();
  return invoke("http_export_collection", { id, dest });
}

/** The environment as a Postman environment file at `dest`, its secrets empty. */
export async function exportHttpEnvironment(id: string, dest: string): Promise<void> {
  requireBackend();
  return invoke("http_export_environment", { id, dest });
}

/** The environments and globals, a secret's value never — `•••` in its place. */
export async function listHttpEnvironments(): Promise<{ environments: Environment[]; globals: EnvVariable[] }> {
  requireBackend();
  return invoke("http_environment_list");
}

/** The form saved: a secret left `•••` or empty keeps its value; a new id is a new environment. */
export async function saveHttpEnvironment(environment: Environment): Promise<void> {
  requireBackend();
  return invoke("http_environment_save", { environment });
}

export async function deleteHttpEnvironment(id: string): Promise<void> {
  requireBackend();
  return invoke("http_environment_delete", { id });
}

/** Every send kept, the oldest first. */
export async function listHttpHistory(): Promise<HttpHistoryEntry[]> {
  requireBackend();
  return invoke<HttpHistoryEntry[]>("http_history_list");
}

/** A pasted `curl …` as a Postman `request` object; rejects with what it could not read. */
export async function parseCurl(command: string): Promise<unknown> {
  requireBackend();
  return invoke("http_parse_curl", { command });
}
