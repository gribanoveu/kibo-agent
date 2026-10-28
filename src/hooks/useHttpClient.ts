import { useCallback, useMemo, useRef, useState } from "react";
import { pickSavePath } from "../lib/dialog";
import { exportHttpFile } from "../lib/httpCommands";
import {
  addRequest,
  cutForHistory,
  effectiveAuth,
  findRequest,
  removeNode,
  renameNode,
  replaceRequest,
  requestsUnder,
  resolveRequest,
  toPostman,
  type Collection,
  type Environment,
  type HistoryEntry,
  type HttpRequest,
  type HttpResponse,
} from "../lib/httpClient";
import { DEMO_COLLECTIONS, DEMO_ENVIRONMENTS, DEMO_HISTORY, demoResponse } from "../lib/httpDemo";
import { useStoredState } from "./useStoredState";

/** What a request last got back — a response, or the error that took its place — and when and where. */
export type Saved = { at: number; environment: string; response?: HttpResponse; error?: string };
/** A request's state on the page: its last answer, whether it is out now, and a refusal before sending. */
export type Sent = Partial<Saved> & { busy: boolean; notSent?: string };

const isStrings = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === "string");
const isActive = (v: unknown): v is string | null => v === null || typeof v === "string";
const isSaved = (v: unknown): v is Record<string, Saved> =>
  typeof v === "object" && v !== null && !Array.isArray(v) && Object.values(v).every((s) => typeof s === "object" && s !== null && typeof (s as Saved).at === "number");
const isHistory = (v: unknown): v is HistoryEntry[] =>
  Array.isArray(v) && v.every((h) => typeof h === "object" && h !== null && typeof h.id === "string" && typeof h.at === "number");

/**
 * The HTTP page's state: collections, environments, the requests open in
 * tabs, what each got back, the history. Held here until the backend has it
 * (HC-2, HC-3) — then these become reads of its commands, and `send` an
 * invoke whose answer is shown only if it is still the latest.
 *
 * The open tabs, each request's last answer and the history — every send's
 * request and response — outlive a restart (docs/45-http-client.md, decision 4).
 */
export function useHttpClient() {
  const [collections, setCollections] = useState<Collection[]>(DEMO_COLLECTIONS);
  const [environments, setEnvironments] = useState<Environment[]>(DEMO_ENVIRONMENTS);
  const [environmentId, setEnvironmentId] = useState<string>("local");
  const [tabs, setTabs] = useStoredState<string[]>("http.tabs", ["create-order", "list-orders"], isStrings);
  const [active, setActive] = useStoredState<string | null>("http.active", "create-order", isActive);
  // ponytail: the window's storage, in plain text, for the sample stand — the
  // plan keeps them sealed on the backend (`responses/<id>.enc`, HC-2), which
  // replaces this.
  const [saved, setSaved] = useStoredState<Record<string, Saved>>("http.responses", {}, isSaved);
  const [history, setHistory] = useStoredState<HistoryEntry[]>("http.history", DEMO_HISTORY, isHistory);
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [notSent, setNotSent] = useState<Record<string, string>>({});
  // A send per request: a second press while the first is out wins, and the first's answer is dropped.
  const latest = useRef<Record<string, number>>({});

  const environment = environments.find((e) => e.id === environmentId) ?? null;

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

  const edit = useCallback((request: HttpRequest) => setCollections((c) => replaceRequest(c, request)), []);

  /** A new, empty collection; its id, to name it. */
  const newCollection = useCallback(() => {
    const id = crypto.randomUUID();
    setCollections((c) => [...c, { id, name: "New collection", auth: { type: "noauth" }, variables: [], items: [], scripts: {} }]);
    return id;
  }, []);

  /** A new GET in a collection or folder, open in a tab; its id, to name it. */
  const newRequest = useCallback(
    (parent: string) => {
      const id = crypto.randomUUID();
      const request: HttpRequest = { id, name: "New request", method: "GET", url: "", headers: [], auth: { type: "inherit" }, body: { mode: "none" }, scripts: {} };
      setCollections((c) => addRequest(c, parent, request));
      // Not through `open`: it checks the collections this render holds, which do not have it yet.
      setTabs((t) => [...t, id]);
      setActive(id);
      return id;
    },
    [setTabs, setActive],
  );

  const rename = useCallback((id: string, name: string) => setCollections((c) => renameNode(c, id, name)), []);

  /**
   * A collection, folder or request gone — its requests' tabs and last answers with it.
   * A send still out is dropped: its answer has no request to go to.
   */
  const remove = useCallback(
    (id: string) => {
      const gone = new Set(requestsUnder(collections, id));
      const left = tabs.filter((t) => !gone.has(t));
      const keep = <T,>(all: Record<string, T>) => Object.fromEntries(Object.entries(all).filter(([request]) => !gone.has(request)));
      setCollections((c) => removeNode(c, id));
      setTabs(left);
      if (active && gone.has(active)) setActive(left[0] ?? null);
      setSaved(keep);
      setBusy(keep);
      setNotSent(keep);
      for (const request of gone) delete latest.current[request];
    },
    [collections, tabs, active, setTabs, setActive, setSaved],
  );

  /** The collection as a Postman file, where the save dialog says; false when cancelled. Throws what the write said. */
  const exportCollection = useCallback(
    async (id: string) => {
      const collection = collections.find((c) => c.id === id);
      if (!collection) return false;
      const dest = await pickSavePath(`${collection.name}.postman_collection`, "json");
      if (!dest) return false;
      await exportHttpFile(dest, toPostman(collection));
      return true;
    },
    [collections],
  );

  const send = useCallback(
    (id: string) => {
      const found = findRequest(collections, id);
      if (!found) return;
      const { request, collection } = found;
      // As it leaves, a secret masked: what history keeps, and what decides a refusal.
      const out = resolveRequest(collection, request, environment);
      // Refused before sending: nothing came back, so the last answer stays.
      const refuse = (why: string) => setNotSent((n) => ({ ...n, [id]: why }));
      if (out.missing.length > 0) return refuse(`No variable ${out.missing.map((m) => `{{${m}}}`).join(", ")} in ${environment?.name ?? "the collection"} — nothing was sent.`);
      if (request.body.mode === "other") return refuse(`A ${request.body.name} body is kept from Postman but not sent yet — it comes with HC-6.`);
      const auth = effectiveAuth(collection, id)?.auth;
      if (auth?.type === "other") return refuse(`${auth.name} auth is kept from Postman but not sent yet — it comes with HC-6. Pick another type to send it.`);
      const ticket = (latest.current[id] ?? 0) + 1;
      latest.current[id] = ticket;
      setNotSent(({ [id]: _, ...rest }) => rest);
      setBusy((b) => ({ ...b, [id]: true }));
      const response = demoResponse(request, out.url);
      const where = environment?.name ?? "";
      window.setTimeout(() => {
        // A later send, or the request deleted meanwhile.
        if (latest.current[id] !== ticket) return;
        setBusy((b) => ({ ...b, [id]: false }));
        setSaved((s) => ({ ...s, [id]: { at: Date.now(), environment: where, response } }));
        const entry: HistoryEntry = {
          id: crypto.randomUUID(),
          at: Date.now(),
          method: request.method,
          url: request.url,
          status: response.status,
          ms: response.ms,
          environment: where,
          requestId: id,
          request: { url: out.url, headers: out.headers, body: out.body && cutForHistory(out.body) },
          response: { ...response, body: cutForHistory(response.body) },
        };
        setHistory((h) => [entry, ...h].slice(0, 200));
      }, response.ms + 250);
    },
    [collections, environment, setSaved, setHistory],
  );

  const sent = useMemo(() => {
    const ids = new Set([...Object.keys(saved), ...Object.keys(busy), ...Object.keys(notSent)]);
    return Object.fromEntries([...ids].map((id) => [id, { ...saved[id], busy: busy[id] ?? false, notSent: notSent[id] } satisfies Sent]));
  }, [saved, busy, notSent]);

  const saveEnvironment = useCallback((next: Environment) => setEnvironments((all) => all.map((e) => (e.id === next.id ? next : e))), []);

  return useMemo(
    () => ({ collections, environments, environment, setEnvironmentId, history, tabs, active, setActive, open, close, edit, newCollection, newRequest, rename, remove, exportCollection, send, sent, saveEnvironment }),
    [collections, environments, environment, history, tabs, active, setActive, open, close, edit, newCollection, newRequest, rename, remove, exportCollection, send, sent, saveEnvironment],
  );
}
