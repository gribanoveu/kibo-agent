import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { pickFile, pickSavePath } from "../lib/dialog";
import {
  addHttpItem,
  applyHttpDraft,
  deleteHttpCollection,
  deleteHttpEnvironment,
  deleteHttpItem,
  moveHttpItem,
  exportHttpCollection,
  exportHttpEnvironment,
  httpEnvironmentSecret,
  importHttpFile,
  lastHttpResponse,
  listHttpCollections,
  listHttpEnvironments,
  httpScripts,
  httpDrafts,
  listHttpHistory,
  newHttpCollection,
  parseCurl,
  rejectHttpDraft,
  renameHttpItem,
  saveHttpEnvironment,
  saveHttpRequest,
  saveHttpScript,
  sendHttpRequest,
  setHttpScripts,
  type HttpHistoryEntry,
  type HttpScriptState,
  type HttpScriptDraft,
  type HttpImported,
  type HttpResponseView,
  onHttpChanged,
} from "../lib/httpCommands";
import {
  addItem,
  addRequest,
  collectionFrom,
  findRequest,
  moveNode,
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
export function useHttpClient(historyShown = false) {
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
  // Each collection's scripts and whether they run, by collection id (HC-11a).
  const [scripts, setScripts] = useState<Record<string, HttpScriptState[]>>({});
  // The model's script drafts waiting for the user (HC-11d).
  const [drafts, setDrafts] = useState<HttpScriptDraft[]>([]);
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
      // One that does not read has none to show.
      const states = await Promise.all(
        list.map((c) =>
          c.error === null
            ? httpScripts(c.id).catch((e) => {
                setProblem(`The scripts were not read: ${e}`);
                return [];
              })
            : [],
        ),
      );
      setScripts(Object.fromEntries(list.map((c, i) => [c.id, states[i]])));
    } catch (e) {
      setProblem(`The collections did not load: ${e}`);
    }
  }, [reloadEnvironments]);

  /** A collection's scripts read again, after the page changed them. */
  const readScripts = useCallback(async (collection: string) => {
    try {
      const states = await httpScripts(collection);
      setScripts((all) => ({ ...all, [collection]: states }));
    } catch (e) {
      setProblem(`The scripts were not read: ${e}`);
    }
  }, []);


  useEffect(() => {
    void reload();
  }, [reload]);

  const readDrafts = useCallback(async () => {
    try {
      setDrafts(await httpDrafts());
    } catch (e) {
      setProblem(`The drafts were not read: ${e}`);
    }
  }, []);

  // What a script wrote. Variables alone: the tree holds edits not saved yet.
  // ponytail: reads every collection for the one that changed; a command for the
  // variables alone when a send's pause shows on many collections.
  const reloadVariables = useCallback(async () => {
    try {
      const [list] = await Promise.all([listHttpCollections(), reloadEnvironments()]);
      const fresh = new Map(list.map(collectionFrom).map((c) => [c.id, c.variables]));
      setCollections((all) => all.map((c) => ({ ...c, variables: fresh.get(c.id) ?? c.variables })));
    } catch (e) {
      setProblem(`The variables did not load: ${e}`);
    }
  }, [reloadEnvironments]);

  // A draft arrives from a chat, and goes with it, on `http:changed`: read once listening,
  // so that one landing in between is in it. So do the variables an agent's send wrote —
  // the environment's form saves over what the page last read.
  useEffect(() => {
    const listening = onHttpChanged(() => {
      void readDrafts();
      void reloadVariables();
    });
    void listening.then(() => readDrafts());
    return () => void listening.then((stop) => stop());
  }, [readDrafts, reloadVariables]);

  // The page's writes, one after another in the order made: a rename right after
  // adding goes after the add, and two saves of one request land in their order.
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const write = useCallback((what: string, run: () => Promise<unknown>) => {
    const next = queue.current.then(run).catch((e) => setProblem(`${what}: ${e}`));
    queue.current = next;
    return next;
  }, []);

  // Edits waiting to be written, by request; a request's own scripts, by request and kind.
  const pending = useRef(new Map<string, { collection: string; request: HttpRequest }>());
  const pendingScripts = useRef(new Map<string, { collection: string; request: string; listen: "prerequest" | "test"; code: string }>());
  // What was typed into a request's own script, by `request\nlisten`: the tab shows it over
  // what was last read, which lags the keys, and it outlives the tab being left. Only the
  // page writes a request's own script, so it is what the file has once written.
  const [typed, setTyped] = useState<Record<string, string>>({});
  const timer = useRef<number | undefined>(undefined);
  const flush = useCallback(() => {
    window.clearTimeout(timer.current);
    for (const [id, { collection, request }] of pending.current) {
      void write(`${request.name} was not saved`, () => saveHttpRequest(collection, id, requestToPostman(request)));
    }
    pending.current.clear();
    // Then read again: what a script is now, and whether it is on (decision 5), is the backend's to say.
    const touched = new Set<string>();
    for (const { collection, request, listen, code } of pendingScripts.current.values()) {
      touched.add(collection);
      void write("A script was not saved", () => saveHttpScript(collection, request, listen, code));
    }
    pendingScripts.current.clear();
    for (const collection of touched) void write("The scripts were not read", () => readScripts(collection));
    return queue.current;
  }, [write, readScripts]);
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

  /** A request's own script as typed, written a moment after the last key, as an edit is. */
  const editScript = useCallback(
    (collection: string, request: string, listen: "prerequest" | "test", code: string) => {
      pendingScripts.current.set(`${request}\n${listen}`, { collection, request, listen, code });
      setTyped((t) => ({ ...t, [`${request}\n${listen}`]: code }));
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(flush, SAVE_AFTER_MS);
    },
    [flush],
  );

  /**
   * Scripts turned on or off by the user, then read again: a refusal says they changed since.
   * In the writes' queue, after what was typed: the switch is for the code on the screen.
   */
  const turnScripts = useCallback(
    (collection: string, hashes: string[], on: boolean) => {
      void flush();
      void write(`The scripts were not turned ${on ? "on" : "off"}`, () => setHttpScripts(collection, hashes, on));
      return write("The scripts were not read", () => readScripts(collection));
    },
    [flush, write, readScripts],
  );

  /**
   * A draft moved into its request in place of its own script, and on; then the scripts and
   * the drafts read again. After what was typed: the user compared the draft with the screen.
   */
  const applyDraft = useCallback(
    (draft: HttpScriptDraft) => {
      void flush();
      void write("The draft was not moved into the request", async () => {
        await applyHttpDraft(draft);
        // What the file has now, over what was typed before.
        setTyped((t) => ({ ...t, [`${draft.request}\n${draft.listen}`]: draft.code }));
      });
      void write("The scripts were not read", () => readScripts(draft.collection));
      return write("The drafts were not read", readDrafts);
    },
    [flush, write, readScripts, readDrafts],
  );

  const rejectDraft = useCallback(
    (draft: HttpScriptDraft) => {
      void write("The draft was not rejected", () => rejectHttpDraft(draft));
      return write("The drafts were not read", readDrafts);
    },
    [write, readDrafts],
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

  /** A new, empty folder named `name` in a collection or folder; its id. */
  const newFolder = useCallback(
    (parent: string, name: string) => {
      const owner = ownerOf(collections, parent);
      if (!owner) return null;
      const id = crypto.randomUUID();
      setCollections((c) => addItem(c, parent, { kind: "folder", id, name, auth: { type: "inherit" }, items: [] }));
      void write("The folder was not added", () => addHttpItem(owner.id, parent === owner.id ? null : parent, { id, name, item: [] }));
      return id;
    },
    [collections, write],
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
   * A folder or request, with all it holds, moved last into the collection or folder `to`.
   * Edits still waiting go first, to the collection they were made in.
   */
  const move = useCallback(
    (id: string, to: string) => {
      const [from, into] = [ownerOf(collections, id), ownerOf(collections, to)];
      if (!from || !into) return;
      flush();
      setCollections((c) => moveNode(c, id, to));
      void write("It was not moved", () => moveHttpItem(from.id, id, into.id, to === into.id ? null : to));
    },
    [collections, write, flush],
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

  // While the log is on screen: read once listening, so that a send landing in between is in it.
  useEffect(() => {
    if (!historyShown) return;
    const listening = onHttpChanged(() => void loadHistory());
    void listening.then(() => loadHistory());
    return () => void listening.then((stop) => stop());
  }, [historyShown, loadHistory]);

  const send = useCallback(
    (id: string) => {
      const found = findRequest(collections, id);
      if (!found) return;
      const ticket = (latest.current[id] ?? 0) + 1;
      latest.current[id] = ticket;
      const current = () => latest.current[id] === ticket;
      setNotSent(({ [id]: _, ...rest }) => rest);
      setBusy((b) => ({ ...b, [id]: true }));
      // What was typed and not yet written goes first: a script runs as it is stored.
      flush()
        .then(() => sendHttpRequest(found.collection.id, id, requestToPostman(found.request), environment?.id ?? null))
        .then((view) => {
          // A send overtaken wrote its variables all the same.
          if (view.scripts.changed) void reloadVariables();
          if (current()) setSaved((s) => ({ ...s, [id]: view }));
        })
        .catch((e) => {
          // A pre-request script that failed keeps what it had written.
          void reloadVariables();
          // Refused before sending — a variable nowhere, a body kept from Postman: the last answer stays.
          if (current()) setNotSent((n) => ({ ...n, [id]: String(e) }));
        })
        .finally(() => {
          if (current()) setBusy((b) => ({ ...b, [id]: false }));
          if (historyRead) void loadHistory();
        });
    },
    [collections, environment, flush, historyRead, loadHistory, reloadVariables],
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

  /** A saved secret's value, asked for by the user from the form. */
  const secretOf = useCallback((environment: string, key: string) => httpEnvironmentSecret(environment, key), []);

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
      tabs,
      active,
      setActive,
      open,
      close,
      edit,
      pasteCurl,
      newCollection,
      newFolder,
      newRequest,
      rename,
      remove,
      move,
      exportCollection,
      importFile,
      send,
      sent,
      scripts,
      turnScripts,
      editScript,
      typed,
      drafts,
      applyDraft,
      rejectDraft,
      saveEnvironment,
      secretOf,
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
      tabs,
      active,
      setActive,
      open,
      close,
      edit,
      pasteCurl,
      newCollection,
      newFolder,
      newRequest,
      rename,
      remove,
      move,
      exportCollection,
      importFile,
      send,
      sent,
      scripts,
      turnScripts,
      editScript,
      typed,
      drafts,
      applyDraft,
      rejectDraft,
      saveEnvironment,
      secretOf,
      newEnvironment,
      deleteEnvironment,
      exportEnvironment,
      problem,
      dismissProblem,
    ],
  );
}
