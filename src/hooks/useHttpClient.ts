import { useCallback, useMemo, useRef, useState } from "react";
import {
  cutForHistory,
  findRequest,
  replaceRequest,
  resolveRequest,
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

  const open = useCallback(
    (id: string) => {
      setTabs((t) => (t.includes(id) ? t : [...t, id]));
      setActive(id);
    },
    [setTabs, setActive],
  );

  const close = useCallback(
    (id: string) => {
      setTabs((t) => {
        const next = t.filter((x) => x !== id);
        if (active === id) setActive(next[t.indexOf(id) - 1] ?? next[0] ?? null);
        return next;
      });
    },
    [active, setTabs, setActive],
  );

  const edit = useCallback((request: HttpRequest) => setCollections((c) => replaceRequest(c, request)), []);

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
      const ticket = (latest.current[id] ?? 0) + 1;
      latest.current[id] = ticket;
      setNotSent(({ [id]: _, ...rest }) => rest);
      setBusy((b) => ({ ...b, [id]: true }));
      const response = demoResponse(request, out.url);
      const where = environment?.name ?? "";
      window.setTimeout(() => {
        if (latest.current[id] !== ticket) return;
        setBusy((b) => ({ ...b, [id]: false }));
        setSaved((s) => ({ ...s, [id]: { at: Date.now(), environment: where, response } }));
        const entry: HistoryEntry = {
          id: `h${Date.now()}`,
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
    () => ({ collections, environments, environment, setEnvironmentId, history, tabs, active, setActive, open, close, edit, send, sent, saveEnvironment }),
    [collections, environments, environment, history, tabs, active, setActive, open, close, edit, send, sent, saveEnvironment],
  );
}
