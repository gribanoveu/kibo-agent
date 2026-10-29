import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { pickFile, pickSavePath } from "../lib/dialog";
import {
  addHttpItem,
  deleteHttpCollection,
  deleteHttpEnvironment,
  deleteHttpItem,
  exportHttpCollection,
  exportHttpEnvironment,
  importHttpFile,
  lastHttpResponse,
  listHttpCollections,
  listHttpEnvironments,
  listHttpHistory,
  newHttpCollection,
  parseCurl,
  renameHttpItem,
  saveHttpEnvironment,
  saveHttpRequest,
  sendHttpRequest,
  type HttpHistoryEntry,
  type HttpImported,
  type HttpResponseView,
} from "../lib/httpCommands";
import {
  addRequest,
  collectionFrom,
  findRequest,
  ownerOf,
  removeNode,
  renameNode,
  replaceRequest,
  requestFrom,
  requestsUnder,
  requestToPostman,
  type Collection,
  type EnvVariable,
  type Environment,
  type HttpRequest,
} from "../lib/httpClient";
import { useStoredState } from "./useStoredState";

/** A request's state on the page: its last answer, whether it is out now, and a refusal before sending. */
export type Sent = { busy: boolean; view?: HttpResponseView; notSent?: string };

const isStrings = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === "string");
const isId = (v: unknown): v is string | null => v === null || typeof v === "string";

/** An edit is written this long after the last keystroke, not on each. */
const SAVE_AFTER_MS = 400;

/**
 * The HTTP page's state over the backend's stores (docs/45-http-client.md):
 * collections, environments, the requests open in tabs, what each got back,
 * the history. The open tabs outlive a restart here; a request's last answer
 * and the history are on the backend, read when its tab or the sheet opens.
 */
export function useHttpClient() {
  const [collections, setCollections] = useState<Collection[]>([]);
  const [environments, setEnvironments] = useState<Environment[]>([]);
  const [globals, setGlobals] = useState<EnvVariable[]>([]);
  const [environmentId, setEnvironmentId] = useStoredState<string | null>("http.environment", null, isId);
  const [tabs, setTabs] = useStoredState<string[]>("http.tabs", [], isStrings);
  const [active, setActive] = useStoredState<string | null>("http.active", null, isId);
  // A request's last response, null when it has none — absent until its tab asks.
  const [saved, setSaved] = useState<Record<string, HttpResponseView | null>>({});
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [notSent, setNotSent] = useState<Record<string, string>>({});
  const [history, setHistory] = useState<HttpHistoryEntry[] | null>(null);
  /** What failed out of sight — a save, a load — for the page to say. */
  const [problem, setProblem] = useState<string | null>(null);
  // A send per request: a second press while the first is out wins, and the first's answer is dropped.
  const latest = useRef<Record<string, number>>({});

  const environment = environments.find((e) => e.id === environmentId) ?? null;

  const reloadEnvironments = useCallback(async () => {
    const all = await listHttpEnvironments();
    setEnvironments(all.environments);
    setGlobals(all.globals);
  }, []);

  const reload = useCallback(async () => {
    try {
      const [list] = await Promise.all([listHttpCollections(), reloadEnvironments()]);
      setCollections(list.map(collectionFrom));
    } catch (e) {
      setProblem(`The collections did not load: ${e}`);
    }
  }, [reloadEnvironments]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // The page's writes, one after another in the order made: a rename right after
  // adding goes after the add, and two saves of one request land in their order.
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const write = useCallback((what: string, run: () => Promise<unknown>) => {
    const next = queue.current.then(run).catch((e) => setProblem(`${what}: ${e}`));
    queue.current = next;
    return next;
  }, []);

  // Edits waiting to be written, by request.
  const pending = useRef(new Map<string, { collection: string; request: HttpRequest }>());
  const timer = useRef<number | undefined>(undefined);
  const flush = useCallback(() => {
    window.clearTimeout(timer.current);
    for (const [id, { collection, request }] of pending.current) {
      void write(`${request.name} was not saved`, () => saveHttpRequest(collection, id, requestToPostman(request)));
    }
    pending.current.clear();
    return queue.current;
  }, [write]);
  // What was typed last before the page closes is written too.
  useEffect(
    () => () => {
      void flush();
    },
    [flush],
  );

  // A tab's saved response is read once, when it is first shown — not again on each edit while it is read.
  const asked = useRef(new Set<string>());
  useEffect(() => {
    if (!active || active in saved || asked.current.has(active)) return;
    const found = findRequest(collections, active);
    if (!found) return;
    asked.current.add(active);
    lastHttpResponse(found.collection.id, active).then(
      // A send that answered meanwhile is newer.
      (view) => setSaved((s) => (active in s ? s : { ...s, [active]: view })),
      (e) => setNotSent((n) => ({ ...n, [active]: `The saved response did not open: ${e}` })),
    );
  }, [active, collections, saved]);

  // Only a request there is: a deleted one's id, from the history, opens nothing.
  const open = useCallback(
    (id: string) => {
      if (!findRequest(collections, id)) return;
      setTabs((t) => (t.includes(id) ? t : [...t, id]));
      setActive(id);
    },
    [collections, setTabs, setActive],
  );

  const close = useCallback(
    (id: string) => {
      const next = tabs.filter((x) => x !== id);
      setTabs(next);
      if (active === id) setActive(next[tabs.indexOf(id) - 1] ?? next[0] ?? null);
    },
    [tabs, active, setTabs, setActive],
  );

  const edit = useCallback(
    (request: HttpRequest) => {
      const owner = ownerOf(collections, request.id);
      if (!owner) return;
      setCollections((c) => replaceRequest(c, request));
      pending.current.set(request.id, { collection: owner.id, request });
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(flush, SAVE_AFTER_MS);
    },
    [collections, flush],
  );

  /** A pasted `curl …` read into the request in place of what it had; throws what it could not read. */
  const pasteCurl = useCallback(
    async (request: HttpRequest, command: string) => {
      const read = requestFrom(request.id, request.name, await parseCurl(command));
      edit({ ...request, method: read.method, url: read.url, paramsOff: read.paramsOff, headers: read.headers, auth: read.auth, body: read.body });
    },
    [edit],
  );

  /** A new, empty collection named `name`; its id. */
  const newCollection = useCallback(async (name: string) => {
    const made = collectionFrom(await newHttpCollection(name));
    setCollections((c) => [...c, made]);
    return made.id;
  }, []);

  /** A new GET named `name` in a collection or folder, open in a tab; its id. */
  const newRequest = useCallback(
    (parent: string, name: string) => {
      const owner = ownerOf(collections, parent);
      if (!owner) return null;
      const id = crypto.randomUUID();
      const request: HttpRequest = { id, name, method: "GET", url: "", headers: [], auth: { type: "inherit" }, body: { mode: "none" }, scripts: {} };
      setCollections((c) => addRequest(c, parent, request));
      void write("The request was not added", () => addHttpItem(owner.id, parent === owner.id ? null : parent, { id, name: request.name, request: requestToPostman(request) }));
      // Not through `open`: it checks the collections this render holds, which do not have it yet.
      setTabs((t) => [...t, id]);
      setActive(id);
      return id;
    },
    [collections, write, setTabs, setActive],
  );

  const rename = useCallback(
    (id: string, name: string) => {
      const owner = ownerOf(collections, id);
      if (!owner) return;
      setCollections((c) => renameNode(c, id, name));
      void write(`It was not renamed`, () => renameHttpItem(owner.id, id, name));
    },
    [collections, write],
  );

  /**
   * A collection, folder or request gone — its requests' tabs and last answers with it.
   * A send still out is dropped: its answer has no request to go to.
   */
  const remove = useCallback(
    (id: string) => {
      const owner = ownerOf(collections, id);
      if (!owner) return;
      const gone = new Set(requestsUnder(collections, id));
      const left = tabs.filter((t) => !gone.has(t));
      const keep = <T,>(all: Record<string, T>) => Object.fromEntries(Object.entries(all).filter(([request]) => !gone.has(request)));
      setCollections((c) => removeNode(c, id));
      setTabs(left);
      if (active && gone.has(active)) setActive(left[0] ?? null);
      setSaved(keep);
      setBusy(keep);
      setNotSent(keep);
      for (const request of gone) {
        delete latest.current[request];
        pending.current.delete(request);
      }
      void write("It was not deleted", () => (id === owner.id ? deleteHttpCollection(id) : deleteHttpItem(owner.id, id)));
    },
    [collections, tabs, active, write, setTabs, setActive],
  );

  /** The collection's file where the save dialog says; false when cancelled. Throws what the write said. */
  const exportCollection = useCallback(
    async (id: string) => {
      const collection = collections.find((c) => c.id === id);
      if (!collection) return false;
      const dest = await pickSavePath(`${collection.name}.postman_collection`, "json");
      if (!dest) return false;
      // The file as the page shows it, edits not yet written included.
      await flush();
      await exportHttpCollection(id, dest);
      return true;
    },
    [collections, flush],
  );

  /** A Postman file picked in the open dialog, into its store; null when cancelled. Throws what the import said. */
  const importFile = useCallback(async (): Promise<HttpImported | null> => {
    const path = await pickFile("Import a Postman collection, environment or globals");
    if (!path) return null;
    const imported = await importHttpFile(path);
    // What is read back has the edits still being written.
    await flush();
    await reload();
    if (imported.kind === "environment") setEnvironmentId(imported.id);
    return imported;
  }, [flush, reload, setEnvironmentId]);

  const loadHistory = useCallback(async () => {
    try {
      setHistory((await listHttpHistory()).reverse());
    } catch (e) {
      setProblem(`The history did not open: ${e}`);
    }
  }, []);
  const historyRead = history !== null;

  const send = useCallback(
    (id: string) => {
      const found = findRequest(collections, id);
      if (!found) return;
      const ticket = (latest.current[id] ?? 0) + 1;
      latest.current[id] = ticket;
      const current = () => latest.current[id] === ticket;
      setNotSent(({ [id]: _, ...rest }) => rest);
      setBusy((b) => ({ ...b, [id]: true }));
      sendHttpRequest(found.collection.id, id, requestToPostman(found.request), environment?.id ?? null)
        .then(
          (view) => current() && setSaved((s) => ({ ...s, [id]: view })),
          // Refused before sending — a variable nowhere, a body kept from Postman: the last answer stays.
          (e) => current() && setNotSent((n) => ({ ...n, [id]: String(e) })),
        )
        .finally(() => {
          if (current()) setBusy((b) => ({ ...b, [id]: false }));
          if (historyRead) void loadHistory();
        });
    },
    [collections, environment, historyRead, loadHistory],
  );

  const sent = useMemo(() => {
    const ids = new Set([...Object.keys(saved), ...Object.keys(busy), ...Object.keys(notSent)]);
    return Object.fromEntries([...ids].map((id) => [id, { view: saved[id] ?? undefined, busy: busy[id] ?? false, notSent: notSent[id] } satisfies Sent]));
  }, [saved, busy, notSent]);

  /** The form saved; throws what the backend refused — a certificate that does not parse, a mask with nothing behind it. */
  const saveEnvironment = useCallback(
    async (next: Environment) => {
      await saveHttpEnvironment(next);
      await reloadEnvironments();
    },
    [reloadEnvironments],
  );

  /** A new environment with an empty `baseUrl`, picked; its id, to edit it. */
  const newEnvironment = useCallback(async () => {
    const id = crypto.randomUUID();
    await saveHttpEnvironment({ id, name: "New environment", variables: [{ key: "baseUrl", value: "", secret: false, enabled: true }] });
    await reloadEnvironments();
    setEnvironmentId(id);
    return id;
  }, [reloadEnvironments, setEnvironmentId]);

  const deleteEnvironment = useCallback(
    async (id: string) => {
      await deleteHttpEnvironment(id);
      await reloadEnvironments();
      if (environmentId === id) setEnvironmentId(null);
    },
    [reloadEnvironments, environmentId, setEnvironmentId],
  );

  /** The environment as a Postman file where the save dialog says, its secrets empty; false when cancelled. */
  const exportEnvironment = useCallback(
    async (id: string) => {
      const env = environments.find((e) => e.id === id);
      if (!env) return false;
      const dest = await pickSavePath(`${env.name}.postman_environment`, "json");
      if (!dest) return false;
      await exportHttpEnvironment(id, dest);
      return true;
    },
    [environments],
  );

  const dismissProblem = useCallback(() => setProblem(null), []);

  return useMemo(
    () => ({
      collections,
      environments,
      environment,
      globals,
      setEnvironmentId,
      history,
      loadHistory,
      tabs,
      active,
      setActive,
      open,
      close,
      edit,
      pasteCurl,
      newCollection,
      newRequest,
      rename,
      remove,
      exportCollection,
      importFile,
      send,
      sent,
      saveEnvironment,
      newEnvironment,
      deleteEnvironment,
      exportEnvironment,
      problem,
      dismissProblem,
    }),
    [
      collections,
      environments,
      environment,
      globals,
      setEnvironmentId,
      history,
      loadHistory,
      tabs,
      active,
      setActive,
      open,
      close,
      edit,
      pasteCurl,
      newCollection,
      newRequest,
      rename,
      remove,
      exportCollection,
      importFile,
      send,
      sent,
      saveEnvironment,
      newEnvironment,
      deleteEnvironment,
      exportEnvironment,
      problem,
      dismissProblem,
    ],
  );
}
