import { useCallback, useEffect, useRef, useState } from "react";
import { useLiveRead } from "./useLiveRead";
import {
  EMPTY_FIND,
  mongoAggregate,
  mongoCollectionCreate,
  mongoCollectionDrop,
  mongoCollections,
  mongoConnectionDelete,
  mongoConnectionSave,
  mongoConnectionTest,
  mongoConnections,
  mongoDatabases,
  mongoDelete,
  mongoFilterAdd,
  mongoCount,
  mongoFind,
  mongoFindClose,
  mongoFindMore,
  mongoIndexCreate,
  mongoIndexDrop,
  mongoIndexes,
  mongoInsert,
  mongoReplace,
  mongoSetFields,
  type CollectionInfo,
  type ConnectionDraft,
  type DatabaseInfo,
  type FieldChange,
  type FindRequest,
  type IndexView,
  type MongoConnection,
  type MongoDocument,
  type MongoTarget,
} from "../lib/mongo";

// No channel: only this page changes what it shows, and each change answers
// with the list after it or reads again.
const unchanging = () => Promise.resolve(() => {});

const keyOf = (target: MongoTarget | null) => (target ? `${target.connection}\u0000${target.database}\u0000${target.collection}` : "");

/**
 * What one view asks the backend: busy while a call runs, its error kept
 * until the next. A read that lands after the view moved on (another
 * collection, another connection) is dropped — `key` says which one it is for.
 */
function useCalls(key: string) {
  const current = useRef(key);
  current.current = key;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setError(null);
    setBusy(false);
  }, [key]);

  /** `work`'s result, or `undefined` when it failed or was for a view since left. */
  const call = useCallback(async <T>(work: () => Promise<T>): Promise<T | undefined> => {
    const asked = current.current;
    setBusy(true);
    setError(null);
    try {
      const result = await work();
      return asked === current.current ? result : undefined;
    } catch (e) {
      if (asked === current.current) setError(String(e));
      return undefined;
    } finally {
      if (asked === current.current) setBusy(false);
    }
  }, []);
  return { busy, error, setError, call };
}

/** The saved connections: read once, and answered with the list by each change. */
export function useMongoConnections() {
  const { value: connections, setValue, error, setError } = useLiveRead(mongoConnections, unchanging, [] as MongoConnection[]);

  /** The list after it, or `null` with the reason in `error`. */
  const change = async (work: () => Promise<MongoConnection[]>): Promise<MongoConnection[] | null> => {
    try {
      const list = await work();
      setValue(list);
      setError(null);
      return list;
    } catch (e) {
      setError(String(e));
      return null;
    }
  };

  return {
    connections,
    error,
    save: (draft: ConnectionDraft) => change(() => mongoConnectionSave(draft)),
    remove: async (id: string) => (await change(() => mongoConnectionDelete(id))) !== null,
    clearError: () => setError(null),
    /** The server's version, or why it could not be reached. */
    test: async (draft: ConnectionDraft): Promise<{ ok: true; version: string } | { ok: false; error: string }> => {
      try {
        return { ok: true, version: await mongoConnectionTest(draft) };
      } catch (e) {
        return { ok: false, error: String(e) };
      }
    },
  };
}

/** A connection's databases, and the collections of those opened. */
export function useMongoTree(connection: string | null) {
  const { busy, error, call } = useCalls(connection ?? "");
  const [databases, setDatabases] = useState<DatabaseInfo[]>([]);
  const [collections, setCollections] = useState<Record<string, CollectionInfo[]>>({});
  const [open, setOpen] = useState<string[]>([]);

  const loadCollections = useCallback(
    async (database: string) => {
      if (!connection) return;
      const found = await call(() => mongoCollections(connection, database));
      if (found) setCollections((all) => ({ ...all, [database]: found }));
    },
    [connection, call],
  );

  const refresh = useCallback(
    async (opened: string[]) => {
      if (!connection) return;
      const found = await call(() => mongoDatabases(connection));
      if (!found) return;
      setDatabases(found);
      for (const database of opened.filter((d) => found.some((f) => f.name === d))) await loadCollections(database);
    },
    [connection, call, loadCollections],
  );

  useEffect(() => {
    setDatabases([]);
    setCollections({});
    setOpen([]);
    void refresh([]);
  }, [connection]);

  return {
    databases,
    collections,
    open,
    busy,
    error,
    refresh: () => refresh(open),
    toggle: (database: string) => {
      const opening = !open.includes(database);
      setOpen(opening ? [...open, database] : open.filter((d) => d !== database));
      if (opening && !collections[database]) void loadCollections(database);
    },
    /** Opens the database it went into. */
    createCollection: async (database: string, collection: string) => {
      if (!connection) return false;
      const done = await call(() => mongoCollectionCreate({ connection, database, collection }).then(() => true));
      if (!done) return false;
      const opened = open.includes(database) ? open : [...open, database];
      setOpen(opened);
      await refresh(opened);
      return true;
    },
    dropCollection: async (database: string, collection: string) => {
      if (!connection) return false;
      const done = await call(() => mongoCollectionDrop({ connection, database, collection }).then(() => true));
      if (done) await refresh(open);
      return !!done;
    },
  };
}

/**
 * A collection's documents: the query bar's last find, read a page at a time
 * from a find the backend keeps open, so the next page goes on where the last
 * stopped. Pages read are kept — going back asks for nothing. The total is
 * counted only when asked: on a view it runs the whole pipeline.
 */
export function useMongoDocuments(target: MongoTarget | null) {
  const key = keyOf(target);
  const { busy, error, setError, call } = useCalls(key);
  const current = useRef(key);
  current.current = key;
  // `skip` is where the find began; its pages are read from there.
  const [request, setRequest] = useState<FindRequest>(EMPTY_FIND);
  const [read, setRead] = useState<{ pages: MongoDocument[][]; at: number; next: number | null } | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [counting, setCounting] = useState(false);
  // The find kept open for the next page: let go when another replaces it, or the view goes.
  const open = useRef<number | null>(null);
  const letGo = () => {
    // Nothing to tell: a find already gone on the backend is what this asks for.
    if (open.current !== null) void mongoFindClose(open.current).catch(() => {});
    open.current = null;
  };

  const find = useCallback(
    async (next: FindRequest) => {
      if (!target) return;
      setRequest(next);
      setTotal(null);
      const found = await call(() => mongoFind(target, next));
      if (!found) return;
      letGo();
      open.current = found.next;
      setRead({ pages: [found.documents], at: 0, next: found.next });
    },
    [key, call],
  );

  useEffect(() => {
    setRead(null);
    setRequest(EMPTY_FIND);
    setTotal(null);
    if (target) void find(EMPTY_FIND);
    return letGo;
  }, [key]);

  const at = read?.at ?? 0;
  /** Where the page shown starts. */
  const skip = request.skip + at * request.limit;

  /** A change, then the same find again from the page shown: it shows what is there now. */
  const change = async (work: (target: MongoTarget) => Promise<unknown>) => {
    if (!target) return false;
    const done = await call(() => work(target).then(() => true));
    if (done) await find({ ...request, skip });
    return !!done;
  };

  return {
    request,
    /** The page shown, `null` before the first is read. */
    documents: read?.pages[at] ?? null,
    skip,
    hasNext: !!read && (at + 1 < read.pages.length || read.next !== null),
    hasPrevious: at > 0 || request.skip > 0,
    total,
    counting,
    busy,
    error,
    clearError: () => setError(null),
    find,
    next: async () => {
      if (!read) return;
      if (at + 1 < read.pages.length) return setRead({ ...read, at: at + 1 });
      const from = read.next;
      if (from === null) return;
      const found = await call(() => mongoFindMore(from));
      if (!found) return;
      // `from` was used up by reading it; the backend holds only `found.next` now.
      open.current = found.next;
      setRead((was) => was && { pages: [...was.pages, found.documents], at: was.pages.length, next: found.next });
    },
    previous: () => {
      if (at > 0) return setRead(read && { ...read, at: at - 1 });
      // Before where the find began: a find of its own from there.
      if (request.skip > 0) void find({ ...request, skip: Math.max(0, request.skip - request.limit) });
    },
    count: async () => {
      if (!target) return;
      const asked = key;
      setCounting(true);
      try {
        const counted = await mongoCount(target, request.filter);
        if (asked === current.current) setTotal(counted);
      } catch (e) {
        if (asked === current.current) setError(String(e));
      } finally {
        if (asked === current.current) setCounting(false);
      }
    },
    insert: (text: string) => change((t) => mongoInsert(t, text)),
    replace: (id: unknown, text: string) => change((t) => mongoReplace(t, id, text)),
    setFields: (id: unknown, changes: FieldChange[]) => change((t) => mongoSetFields(t, id, changes)),
    remove: (id: unknown) => change((t) => mongoDelete(t, id)),
    /** The filter with a field's condition added, or `undefined` with the reason in `error`. */
    condition: (filter: string, path: string[], text: string, exclude: boolean) => call(() => mongoFilterAdd(filter, path, text, exclude)),
  };
}

/** An aggregation run on the collection: its results, kept until another runs or the collection changes. */
export function useMongoAggregation(target: MongoTarget | null) {
  const key = keyOf(target);
  const { busy, error, call } = useCalls(key);
  const [results, setResults] = useState<MongoDocument[] | null>(null);
  useEffect(() => setResults(null), [key]);
  return {
    results,
    busy,
    error,
    run: async (pipeline: string) => {
      if (!target) return;
      const found = await call(() => mongoAggregate(target, pipeline));
      if (found) setResults(found);
    },
  };
}

/** A collection's indexes, read while the tab is showing. */
export function useMongoIndexes(target: MongoTarget | null) {
  const key = keyOf(target);
  const { busy, error, call } = useCalls(key);
  const [indexes, setIndexes] = useState<IndexView[]>([]);

  const load = useCallback(async () => {
    if (!target) return;
    const found = await call(() => mongoIndexes(target));
    if (found) setIndexes(found);
  }, [key, call]);

  useEffect(() => {
    setIndexes([]);
    void load();
  }, [key]);

  return {
    indexes,
    busy,
    error,
    create: async (keys: string, unique: boolean, name: string) => {
      if (!target) return false;
      const made = await call(() => mongoIndexCreate(target, keys, unique, name.trim() || null));
      if (made === undefined) return false;
      await load();
      return true;
    },
    drop: async (name: string) => {
      if (!target) return false;
      const done = await call(() => mongoIndexDrop(target, name).then(() => true));
      if (done) await load();
      return !!done;
    },
  };
}
