// The HTTP page's shapes and the pure rules over them (docs/45-http-client.md).
// Collections follow Postman Collection v2.1 — the subset the page edits; the
// rest of a real file stays on the backend, untouched (decision 2). No command
// exists yet: the page runs on the sample in `httpDemo.ts` until HC-3.

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS";
export const METHODS: HttpMethod[] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/** A header, a query parameter, a form field or a variable: Postman's `{key, value, disabled}`. */
export type Pair = { key: string; value: string; disabled?: boolean };

export type Auth =
  | { type: "inherit" }
  | { type: "noauth" }
  | { type: "bearer"; token: string }
  | { type: "basic"; username: string; password: string }
  | { type: "apikey"; key: string; value: string; in: "header" | "query" }
  /** A type the page does not edit (`oauth2`, `digest`…), kept as it came. */
  | { type: "other"; name: string };

export type Body =
  | { mode: "none" }
  | { mode: "raw"; language: "json" | "text" | "xml"; raw: string }
  | { mode: "urlencoded"; fields: Pair[] }
  /** `formdata`, `file`, `graphql` until HC-6: shown, kept, not sent. */
  | { mode: "other"; name: string };

export type Scripts = { prerequest?: string; test?: string };

export type HttpRequest = {
  id: string;
  name: string;
  method: HttpMethod;
  url: string;
  headers: Pair[];
  auth: Auth;
  body: Body;
  scripts: Scripts;
};

export type Folder = { id: string; name: string; auth: Auth; items: Item[] };
export type Item = ({ kind: "folder" } & Folder) | ({ kind: "request" } & HttpRequest);

export type Collection = { id: string; name: string; auth: Auth; variables: Pair[]; items: Item[]; scripts: Scripts };

/** What the agent may do in an environment (decision 6). */
export type AgentAccess = "off" | "read" | "ask" | "allow";
export const AGENT_ACCESS: { value: AgentAccess; label: string; hint: string }[] = [
  { value: "off", label: "Off", hint: "The agent does not see this environment" },
  { value: "read", label: "Read only", hint: "GET, HEAD, OPTIONS — anything else is refused" },
  { value: "ask", label: "Ask for writes", hint: "Every write asks, even under Always allow and Auto" },
  { value: "allow", label: "Writes as other tools", hint: "Writes ask as other tools do" },
];

/** A variable as the window sees it: a secret's value never comes back, only that it has one. */
export type EnvVariable = { key: string; value: string; secret: boolean; enabled: boolean };
export type Environment = { id: string; name: string; variables: EnvVariable[]; agent: AgentAccess; trustedCert: boolean };

export type HttpResponse = {
  status: number;
  statusText: string;
  ms: number;
  bytes: number;
  /** After redirects, when they moved it. */
  finalUrl?: string;
  headers: Pair[];
  body: string;
  contentType: string;
  tests: { name: string; passed: boolean; error?: string }[];
};

export type HistoryEntry = {
  id: string;
  at: number;
  method: HttpMethod;
  /** Before substitution — `{{baseUrl}}`, never a secret's value. */
  url: string;
  status: number | null;
  ms: number;
  environment: string;
  /** The agent's call, with the chat it came from. */
  agent?: { chat: string };
};

export const SECRET_MASK = "•••";

/** `{{name}}` in `text`, in order, each once; Postman's dynamic `{{$…}}` left out. */
export function variablesIn(text: string): string[] {
  const names = [...text.matchAll(/\{\{\s*([^{}\s]+)\s*\}\}/g)].map((m) => m[1]).filter((n) => !n.startsWith("$"));
  return [...new Set(names)];
}

/** Postman's order: the collection's variables, the environment over them. */
export function scope(collection: Collection | null, environment: Environment | null): Map<string, EnvVariable> {
  const out = new Map<string, EnvVariable>();
  for (const v of collection?.variables ?? []) if (!v.disabled) out.set(v.key, { key: v.key, value: v.value, secret: false, enabled: true });
  for (const v of environment?.variables ?? []) if (v.enabled) out.set(v.key, v);
  return out;
}

/** `text` as it would be sent, a secret shown masked — for the line under the address. */
export function preview(text: string, vars: Map<string, EnvVariable>): { text: string; missing: string[] } {
  const missing: string[] = [];
  const out = text.replace(/\{\{\s*([^{}\s]+)\s*\}\}/g, (whole, name: string) => {
    if (name.startsWith("$")) return whole;
    const v = vars.get(name);
    if (!v) {
      missing.push(name);
      return whole;
    }
    return v.secret ? SECRET_MASK : v.value;
  });
  return { text: out, missing: [...new Set(missing)] };
}

/** The query of an address as rows, for the Params tab. */
export function queryOf(url: string): Pair[] {
  const at = url.indexOf("?");
  if (at < 0) return [];
  return url
    .slice(at + 1)
    .split("&")
    .filter((part) => part !== "")
    .map((part) => {
      const eq = part.indexOf("=");
      return eq < 0 ? { key: part, value: "" } : { key: part.slice(0, eq), value: part.slice(eq + 1) };
    });
}

/** The address with its query replaced by `params` — the Params tab writing back. Disabled rows stay out of it. */
export function withQuery(url: string, params: Pair[]): string {
  const at = url.indexOf("?");
  const base = at < 0 ? url : url.slice(0, at);
  const query = params
    .filter(({ key, disabled }) => !disabled && key !== "")
    .map((p) => (p.value === "" ? p.key : `${p.key}=${p.value}`))
    .join("&");
  return query ? `${base}?${query}` : base;
}

/** Where a request's auth comes from when it says "inherit": the nearest folder, then the collection. */
export function effectiveAuth(collection: Collection, requestId: string): { auth: Auth; from: string } | null {
  const walk = (items: Item[], inherited: { auth: Auth; from: string }): { auth: Auth; from: string } | null => {
    for (const item of items) {
      if (item.kind === "request" && item.id === requestId)
        return item.auth.type === "inherit" ? inherited : { auth: item.auth, from: item.name };
      if (item.kind === "folder") {
        const here = item.auth.type === "inherit" ? inherited : { auth: item.auth, from: item.name };
        const found = walk(item.items, here);
        if (found) return found;
      }
    }
    return null;
  };
  const top = collection.auth.type === "inherit" ? { auth: { type: "noauth" } as Auth, from: collection.name } : { auth: collection.auth, from: collection.name };
  return walk(collection.items, top);
}

export function findRequest(collections: Collection[], id: string): { collection: Collection; request: HttpRequest; path: string[] } | null {
  for (const collection of collections) {
    const walk = (items: Item[], path: string[]): { request: HttpRequest; path: string[] } | null => {
      for (const item of items) {
        if (item.kind === "request" && item.id === id) return { request: item, path };
        if (item.kind === "folder") {
          const found = walk(item.items, [...path, item.name]);
          if (found) return found;
        }
      }
      return null;
    };
    const found = walk(collection.items, [collection.name]);
    if (found) return { collection, ...found };
  }
  return null;
}

/** `request` put back in its place in the tree. */
export function replaceRequest(collections: Collection[], request: HttpRequest): Collection[] {
  const swap = (items: Item[]): Item[] =>
    items.map((item) =>
      item.kind === "request" && item.id === request.id
        ? { kind: "request", ...request }
        : item.kind === "folder"
          ? { ...item, items: swap(item.items) }
          : item,
    );
  return collections.map((c) => ({ ...c, items: swap(c.items) }));
}

export const hasScripts = (s: Scripts) => Boolean(s.prerequest?.trim() || s.test?.trim());

/** The tone of a status: the colour token the page draws it in. */
export function statusTone(status: number): "ok" | "redirect" | "client" | "server" {
  if (status >= 500) return "server";
  if (status >= 400) return "client";
  if (status >= 300) return "redirect";
  return "ok";
}

export function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** JSON with indents when it parses, the text as it came when it does not. */
export function pretty(body: string, contentType: string): string {
  if (!/json/i.test(contentType)) return body;
  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return body;
  }
}
