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
  /** The collection's request it was, if it was one — the entry opens it. */
  requestId?: string;
  /** The agent's call, with the chat it came from. */
  agent?: { chat: string };
  /** What left, as Postman's console shows it — a secret masked. Absent in entries from before it was kept. */
  request?: SentRequest;
  /** What came back, its body cut at `HISTORY_BODY_LIMIT`; or `error` in its place. */
  response?: HttpResponse;
  error?: string;
};

/** A request as it left: variables and auth filled in, a secret's value never — `•••`. */
export type SentRequest = { url: string; headers: Pair[]; body: string | null };

/**
 * A body in history is cut here: the log is for reading what happened, and
 * 200 whole bodies would be hundreds of megabytes of someone else's data.
 * The request keeps its last response whole.
 */
export const HISTORY_BODY_LIMIT = 256 * 1024;

export function cutForHistory(body: string): string {
  return body.length <= HISTORY_BODY_LIMIT ? body : `${body.slice(0, HISTORY_BODY_LIMIT)}\n… cut at ${sizeLabel(HISTORY_BODY_LIMIT)} in history`;
}

/** A request or a response as HTTP writes it: the first line, the headers, a blank line, the body. */
export function asHttpText(first: string, headers: Pair[], body: string | null): string {
  return [first, ...headers.map((h) => `${h.key}: ${h.value}`), ...(body ? ["", body] : [])].join("\n");
}

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

/** How a secret is written where it is shown: masked on the page, its `{{name}}` in a copied cURL. */
export type SecretAs = "mask" | "name";

/** `text` with its variables filled in, a secret never by its value. */
export function preview(text: string, vars: Map<string, EnvVariable>, secretAs: SecretAs = "mask"): { text: string; missing: string[] } {
  const missing: string[] = [];
  const out = text.replace(/\{\{\s*([^{}\s]+)\s*\}\}/g, (whole, name: string) => {
    if (name.startsWith("$")) return whole;
    const v = vars.get(name);
    if (!v) {
      missing.push(name);
      return whole;
    }
    return v.secret ? (secretAs === "mask" ? SECRET_MASK : `{{${name}}}`) : v.value;
  });
  return { text: out, missing: [...new Set(missing)] };
}

/** A request as it leaves: variables filled in, auth turned into a header or a parameter, the body as text. */
export type Resolved = {
  method: HttpMethod;
  url: string;
  headers: Pair[];
  body: string | null;
  /** Variables nowhere in scope — the request is not sent while there are any. */
  missing: string[];
  /** What is not sent as written: an auth or a body kept from Postman. */
  notes: string[];
};

const hasHeader = (headers: Pair[], name: string) => headers.some(({ key }) => key.toLowerCase() === name.toLowerCase());

export function resolveRequest(collection: Collection, request: HttpRequest, environment: Environment | null, secretAs: SecretAs = "mask"): Resolved {
  const vars = scope(collection, environment);
  const missing = new Set<string>();
  const fill = (text: string) => {
    const done = preview(text, vars, secretAs);
    done.missing.forEach((m) => missing.add(m));
    return done.text;
  };
  const notes: string[] = [];
  let url = fill(request.url);
  const headers = request.headers.filter(({ key, disabled }) => !disabled && key !== "").map((h) => ({ key: fill(h.key), value: fill(h.value) }));

  const auth = request.auth.type === "inherit" ? (effectiveAuth(collection, request.id)?.auth ?? { type: "noauth" }) : request.auth;
  if (auth.type === "bearer" && !hasHeader(headers, "Authorization")) headers.push({ key: "Authorization", value: `Bearer ${fill(auth.token)}` });
  if (auth.type === "basic" && !hasHeader(headers, "Authorization")) {
    const user = fill(auth.username);
    const password = fill(auth.password);
    // A masked or named secret cannot be encoded — it is shown as it is.
    const plain = !/\{\{|•/.test(`${user}${password}`);
    headers.push({ key: "Authorization", value: plain ? `Basic ${btoa(`${user}:${password}`)}` : `Basic ${user}:${password}` });
  }
  if (auth.type === "apikey") {
    if (auth.in === "header") headers.push({ key: fill(auth.key), value: fill(auth.value) });
    else url += `${url.includes("?") ? "&" : "?"}${fill(auth.key)}=${fill(auth.value)}`;
  }
  if (auth.type === "other") notes.push(`${auth.name} auth is kept from Postman and not sent yet`);

  let body: string | null = null;
  if (request.body.mode === "raw") {
    body = fill(request.body.raw);
    const type = { json: "application/json", text: "text/plain", xml: "application/xml" }[request.body.language];
    if (!hasHeader(headers, "Content-Type")) headers.push({ key: "Content-Type", value: type });
  }
  if (request.body.mode === "urlencoded") {
    body = request.body.fields
      .filter(({ key, disabled }) => !disabled && key !== "")
      .map((f) => `${encodeURIComponent(fill(f.key))}=${encodeURIComponent(fill(f.value))}`)
      .join("&");
    if (!hasHeader(headers, "Content-Type")) headers.push({ key: "Content-Type", value: "application/x-www-form-urlencoded" });
  }
  if (request.body.mode === "other") notes.push(`a ${request.body.name} body is kept from Postman and not sent yet`);

  return { method: request.method, url, headers, body, missing: [...missing], notes };
}

/** One shell word, in single quotes. */
const quote = (text: string) => `'${text.replace(/'/g, `'\\''`)}'`;

/** The request as a cURL command — a secret by its `{{name}}`, so the copy carries no token. */
export function toCurl(r: Resolved): string {
  const lines = [`curl${r.method === "GET" ? "" : ` -X ${r.method}`} ${quote(r.url)}`];
  for (const h of r.headers) lines.push(`-H ${quote(`${h.key}: ${h.value}`)}`);
  if (r.body !== null && r.body !== "") lines.push(`--data-raw ${quote(r.body)}`);
  return lines.join(" \\\n  ");
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
