import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

// The MongoDB page (docs/43-mongodb.md): saved connections, and what is in
// their servers. Mirrors `domain::mongo` and `services::mongo`. Query text is
// mongosh's — `{ age: { $gt: 30 } }`, `ObjectId('…')` — read by the backend.

/** A saved connection: never its URI, only where it goes, the password masked. */
export type MongoConnection = { id: string; name: string; address: string };

/** What the form sends. No `uri` on an edit keeps the saved one. */
export type ConnectionDraft = { id?: string; name: string; uri?: string };

export type DatabaseInfo = { name: string; sizeOnDisk: number };

export type CollectionInfo = { name: string; kind: "collection" | "view" | "timeseries" };

/** A collection, by the connection it is reached through. */
export type MongoTarget = { connection: string; database: string; collection: string };

export type FindRequest = { filter: string; projection: string; sort: string; skip: number; limit: number };

/** A document as mongosh shows it; `id` is its `_id` as the backend gave it, sent back as is. */
export type MongoDocument = { id: unknown | null; text: string };

export type FindPage = { documents: MongoDocument[]; total: number };

export type IndexView = { name: string; keys: string; unique: boolean; sparse: boolean; ttlSeconds: number | null; partial: string | null };

export const EMPTY_FIND: FindRequest = { filter: "", projection: "", sort: "", skip: 0, limit: 20 };

export async function mongoConnections(): Promise<MongoConnection[]> {
  if (!inTauri()) return [];
  return invoke<MongoConnection[]>("mongo_connections");
}

/** The list after it. */
export async function mongoConnectionSave(draft: ConnectionDraft): Promise<MongoConnection[]> {
  requireBackend();
  return invoke<MongoConnection[]>("mongo_connection_save", { draft });
}

export async function mongoConnectionDelete(id: string): Promise<MongoConnection[]> {
  requireBackend();
  return invoke<MongoConnection[]>("mongo_connection_delete", { id });
}

/** The server's version. With no `uri`, the saved connection's. */
export async function mongoConnectionTest(draft: ConnectionDraft): Promise<string> {
  requireBackend();
  return invoke<string>("mongo_connection_test", { draft });
}

export async function mongoDatabases(connection: string): Promise<DatabaseInfo[]> {
  requireBackend();
  return invoke<DatabaseInfo[]>("mongo_databases", { connection });
}

export async function mongoCollections(connection: string, database: string): Promise<CollectionInfo[]> {
  requireBackend();
  return invoke<CollectionInfo[]>("mongo_collections", { connection, database });
}

export async function mongoFind(target: MongoTarget, request: FindRequest): Promise<FindPage> {
  requireBackend();
  return invoke<FindPage>("mongo_find", { target, request });
}

export async function mongoAggregate(target: MongoTarget, pipeline: string): Promise<MongoDocument[]> {
  requireBackend();
  return invoke<MongoDocument[]>("mongo_aggregate", { target, pipeline });
}

/** How many went in: `text` is a document or an array of them. */
export async function mongoInsert(target: MongoTarget, text: string): Promise<number> {
  requireBackend();
  return invoke<number>("mongo_insert", { target, text });
}

export async function mongoReplace(target: MongoTarget, id: unknown, text: string): Promise<void> {
  requireBackend();
  return invoke<void>("mongo_replace", { target, id, text });
}

export async function mongoDelete(target: MongoTarget, id: unknown): Promise<void> {
  requireBackend();
  return invoke<void>("mongo_delete", { target, id });
}

export async function mongoIndexes(target: MongoTarget): Promise<IndexView[]> {
  requireBackend();
  return invoke<IndexView[]>("mongo_indexes", { target });
}

/** The new index's name. */
export async function mongoIndexCreate(target: MongoTarget, keys: string, unique: boolean, name: string | null): Promise<string> {
  requireBackend();
  return invoke<string>("mongo_index_create", { target, keys, unique, name });
}

export async function mongoIndexDrop(target: MongoTarget, name: string): Promise<void> {
  requireBackend();
  return invoke<void>("mongo_index_drop", { target, name });
}

/** In a database not there yet, it makes that too. */
export async function mongoCollectionCreate(target: MongoTarget): Promise<void> {
  requireBackend();
  return invoke<void>("mongo_collection_create", { target });
}

export async function mongoCollectionDrop(target: MongoTarget): Promise<void> {
  requireBackend();
  return invoke<void>("mongo_collection_drop", { target });
}

/** `21–40 of 1,234`: where a page of a find is. */
export function pageRange(skip: number, shown: number, total: number): string {
  if (shown === 0) return `0 of ${total.toLocaleString("en-US")}`;
  return `${(skip + 1).toLocaleString("en-US")}–${(skip + shown).toLocaleString("en-US")} of ${total.toLocaleString("en-US")}`;
}

/** A document's text without its `_id` line: what "Clone" starts from, so the copy gets an `_id` of its own. */
export function withoutId(text: string): string {
  return text
    .split("\n")
    // One line only: an `_id` that is a document of its own spans several, and stays.
    .filter((line) => !/^ {2}_id: .*[^[{]$/.test(line))
    .join("\n")
    .replace(/,(\n\})$/, "$1");
}
