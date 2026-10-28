import { useCallback, useMemo, useRef, useState } from "react";
import {
  findRequest,
  preview,
  replaceRequest,
  scope,
  type Collection,
  type Environment,
  type HistoryEntry,
  type HttpRequest,
  type HttpResponse,
} from "../lib/httpClient";
import { DEMO_COLLECTIONS, DEMO_ENVIRONMENTS, DEMO_HISTORY, demoResponse } from "../lib/httpDemo";

export type Sent = { busy: boolean; response?: HttpResponse; error?: string };

/**
 * The HTTP page's state: collections, environments, the requests open in
 * tabs, what each got back, the history. Held here until the backend has it
 * (HC-2, HC-3) — then these become reads of its commands, and `send` an
 * invoke whose answer is shown only if it is still the latest.
 */
export function useHttpClient() {
  const [collections, setCollections] = useState<Collection[]>(DEMO_COLLECTIONS);
  const [environments, setEnvironments] = useState<Environment[]>(DEMO_ENVIRONMENTS);
  const [environmentId, setEnvironmentId] = useState<string>("local");
  const [history, setHistory] = useState<HistoryEntry[]>(DEMO_HISTORY);
  const [tabs, setTabs] = useState<string[]>(["create-order", "list-orders"]);
  const [active, setActive] = useState<string | null>("create-order");
  const [sent, setSent] = useState<Record<string, Sent>>({});
  // A send per request: a second press while the first is out wins, and the first's answer is dropped.
  const latest = useRef<Record<string, number>>({});

  const environment = environments.find((e) => e.id === environmentId) ?? null;

  const open = useCallback((id: string) => {
    setTabs((t) => (t.includes(id) ? t : [...t, id]));
    setActive(id);
  }, []);

  const close = useCallback(
    (id: string) => {
      setTabs((t) => {
        const next = t.filter((x) => x !== id);
        if (active === id) setActive(next[next.indexOf(id) - 1] ?? next[0] ?? null);
        return next;
      });
    },
    [active],
  );

  const edit = useCallback((request: HttpRequest) => setCollections((c) => replaceRequest(c, request)), []);

  const send = useCallback(
    (id: string) => {
      const found = findRequest(collections, id);
      if (!found) return;
      const { request, collection } = found;
      const vars = scope(collection, environment);
      const url = preview(request.url, vars);
      const fail = (error: string) => setSent((s) => ({ ...s, [id]: { busy: false, error } }));
      if (url.missing.length > 0) return fail(`No variable ${url.missing.map((m) => `{{${m}}}`).join(", ")} in ${environment?.name ?? "the collection"} — nothing was sent.`);
      if (request.body.mode === "other") return fail(`A ${request.body.name} body is kept from Postman but not sent yet — it comes with HC-6.`);
      const ticket = (latest.current[id] ?? 0) + 1;
      latest.current[id] = ticket;
      setSent((s) => ({ ...s, [id]: { ...s[id], busy: true, error: undefined } }));
      const response = demoResponse(request, url.text);
      window.setTimeout(() => {
        if (latest.current[id] !== ticket) return;
        setSent((s) => ({ ...s, [id]: { busy: false, response } }));
        setHistory((h) => [
          { id: `h${Date.now()}`, at: Date.now(), method: request.method, url: request.url, status: response.status, ms: response.ms, environment: environment?.name ?? "" },
          ...h,
        ].slice(0, 200));
      }, response.ms + 250);
    },
    [collections, environment],
  );

  const saveEnvironment = useCallback((next: Environment) => setEnvironments((all) => all.map((e) => (e.id === next.id ? next : e))), []);

  return useMemo(
    () => ({ collections, environments, environment, setEnvironmentId, history, tabs, active, setActive, open, close, edit, send, sent, saveEnvironment }),
    [collections, environments, environment, history, tabs, active, open, close, edit, send, sent, saveEnvironment],
  );
}
