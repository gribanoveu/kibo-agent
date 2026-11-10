// The HTTP page's shapes and the pure rules over them (docs/45-http-client.md).
// Collections are Postman Collection v2.1 files kept by the backend; the page
// reads the subset it edits out of them, and writes an edit back over the
// request it came from, so what it does not edit stays as it was (decision 2).

import { formatJson, formatXml, stripJsonComments } from "./formatText";

/** Any method a file names — `PURGE`, `PROPFIND` — goes; these are the ones offered. */
export type HttpMethod = string;
export const METHODS: HttpMethod[] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/** A header, a query parameter, a form field or a variable: Postman's `{key, value, disabled}`. */
export type Pair = { key: string; value: string; disabled?: boolean };

export type Auth =
  | { type: "inherit" }
  | { type: "noauth" }
  | { type: "bearer"; token: string }
  | { type: "basic"; username: string; password: string }
  | { type: "apikey"; key: string; value: string; in: "header" | "query" }
  /** Postman's own rows: the ones the page does not show — `tokenName`, say — stay in them. */
  | { type: "digest" | "oauth2"; params: Record<string, unknown>[] }
  /** A type the page does not edit (`hawk`, `ntlm`…), kept as it came. */
  | { type: "other"; name: string };

/** A `formdata` row. `file` marks a file field, holding its path; `""` is one never picked. */
export type FormField = Pair & { file?: string };

export type Body =
  | { mode: "none" }
  | { mode: "raw"; language: "json" | "text" | "xml"; raw: string }
  | { mode: "urlencoded"; fields: Pair[] }
  | { mode: "formdata"; fields: FormField[] }
  /** `file`, `graphql`: shown, kept, not sent. */
  | { mode: "other"; name: string };

export type Scripts = { prerequest?: string; test?: string };

export type HttpRequest = {
  id: string;
  name: string;
  method: HttpMethod;
  url: string;
  /** Query parameters turned off in Params: kept beside the address, out of it, as Postman keeps `disabled` ones. */
  paramsOff?: Pair[];
  headers: Pair[];
  auth: Auth;
  body: Body;
  scripts: Scripts;
  /** The Postman `request` it was read from — what the page does not edit goes back from it. None for one made here. */
  source?: unknown;
};

export type Folder = { id: string; name: string; auth: Auth; items: Item[] };
export type Item = ({ kind: "folder" } & Folder) | ({ kind: "request" } & HttpRequest);

/** `error` says why a stored file does not read; such a collection holds nothing and can only be deleted. */
export type Collection = { id: string; name: string; auth: Auth; variables: (Pair & { secret?: boolean })[]; items: Item[]; scripts: Scripts; error?: string };

/** A variable as the window sees it: a secret's value never comes back, only `•••` when it has one. */
export type EnvVariable = { key: string; value: string; secret: boolean; enabled: boolean };
/** How far the agent may go in an environment; absent is off. */
export type AgentAccess = "off" | "readOnly" | "askForWrites" | "writesAsOtherTools";
/** `trustedCert` is the PEM of the stand's own certificate or its CA's. */
export type Environment = { id: string; name: string; variables: EnvVariable[]; trustedCert?: string; agent?: AgentAccess };

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

/** Postman's order: globals, the collection's variables over them, the environment over those. */
export function scope(collection: Collection | null, environment: Environment | null, globals: EnvVariable[] = []): Map<string, EnvVariable> {
  const out = new Map<string, EnvVariable>();
  for (const v of globals) if (v.enabled) out.set(v.key, v);
  for (const v of collection?.variables ?? []) if (!v.disabled) out.set(v.key, { key: v.key, value: v.value, secret: v.secret ?? false, enabled: true });
  for (const v of environment?.variables ?? []) if (v.enabled) out.set(v.key, v);
  return out;
}

/** Each variable in scope by the value the page may show: a secret masked. */
export function shownValues(vars: Map<string, EnvVariable>): Map<string, string> {
  return new Map([...vars].map(([key, v]) => [key, v.secret ? SECRET_MASK : v.value]));
}

/** `{{name}}`, as Postman writes a variable: the name is group 1. */
export const VARIABLE = /\{\{\s*([^{}\s]+)\s*\}\}/g;

/** What `{{name}}` stands for: its value, Postman's dynamic `{{$…}}` made as it is sent — or null, found nowhere. */
export function variableValue(name: string, values: ReadonlyMap<string, string>): string | null {
  if (name.startsWith("$")) return "made as the request is sent";
  return values.get(name) ?? null;
}

/** A `{{` being typed before the caret: where it starts, and what of the name is typed after it. */
export function variableAt(text: string, caret: number): { from: number; typed: string } | null {
  const m = /\{\{\s*([^{}\s]*)$/.exec(text.slice(0, caret));
  return m ? { from: m.index, typed: m[1] } : null;
}

/** The variables in scope for what is typed: names that start with it first, then those that hold it. */
export function variableHints(values: ReadonlyMap<string, string>, typed: string): { name: string; value: string }[] {
  const all = [...values].map(([name, value]) => ({ name, value }));
  const low = typed.toLowerCase();
  const starts = all.filter((v) => v.name.toLowerCase().startsWith(low));
  return [...starts, ...all.filter((v) => !starts.includes(v) && v.name.toLowerCase().includes(low))];
}

/**
 * `text` with `{{name}}` from `from` to the caret — the rest of a name and the `}}`
 * already after the caret replaced rather than doubled — and the caret after it.
 */
export function putVariable(text: string, from: number, caret: number, name: string): { text: string; caret: number } {
  // Name characters only: in `{"id": "{{"}}` the `"}}` after the caret is the JSON's, not a closer.
  const rest = /^[\w.$-]*\}\}/.exec(text.slice(caret))?.[0].length ?? 0;
  const put = `{{${name}}}`;
  return { text: text.slice(0, from) + put + text.slice(caret + rest), caret: from + put.length };
}

export type TextPart = { text: string; variable?: { name: string; value: string | null } };

/** `text` cut at its variables, each with what it stands for — for drawing them found or not. */
export function variableParts(text: string, values: ReadonlyMap<string, string>): TextPart[] {
  const parts: TextPart[] = [];
  let at = 0;
  for (const m of text.matchAll(VARIABLE)) {
    if (m.index > at) parts.push({ text: text.slice(at, m.index) });
    parts.push({ text: m[0], variable: { name: m[1], value: variableValue(m[1], values) } });
    at = m.index + m[0].length;
  }
  if (at < text.length) parts.push({ text: text.slice(at) });
  return parts;
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

/** Where in the request a variable is written — the page marks a missing one there. */
export type Place = "url" | "headers" | "auth" | "body";

/** A request as it leaves: variables filled in, auth turned into a header or a parameter, the body as text. */
export type Resolved = {
  method: HttpMethod;
  url: string;
  headers: Pair[];
  body: string | null;
  /** A `formdata` body's fields: `body` shows them, a cURL names them one by one. */
  form: FormField[] | null;
  /** Variables nowhere in scope — the request is not sent while there are any. */
  missing: string[];
  /** The same, by where each is written. */
  missingIn: Record<Place, string[]>;
  /** What is not sent as written: an auth or a body kept from Postman. */
  notes: string[];
};

/** A `digest` or `oauth2` setting by Postman's name for it; empty when it has none. */
export function authParam(auth: { params: Record<string, unknown>[] }, key: string): string {
  return text(auth.params.find((p) => key === p.key)?.value);
}

/** The auth with one setting changed, the rows it does not name as they were. */
export function withAuthParam<A extends { params: Record<string, unknown>[] }>(auth: A, key: string, value: string): A {
  const has = auth.params.some((p) => key === p.key);
  return { ...auth, params: has ? auth.params.map((p) => (key === p.key ? { ...p, value } : p)) : [...auth.params, { key, value, type: "string" }] };
}

/** Whether Kibo gets the token itself as the request goes; anything else needs a browser. */
export function fetchesToken(auth: { params: Record<string, unknown>[] }): boolean {
  return fetchingGrant(auth) && authParam(auth, "accessTokenUrl") !== "";
}

const fetchingGrant = (auth: { params: Record<string, unknown>[] }) => ["client_credentials", "password_credentials"].includes(authParam(auth, "grant_type"));

const hasHeader = (headers: Pair[], name: string) => headers.some(({ key }) => key.toLowerCase() === name.toLowerCase());

export function resolveRequest(collection: Collection, request: HttpRequest, environment: Environment | null, secretAs: SecretAs = "mask", globals: EnvVariable[] = []): Resolved {
  const vars = scope(collection, environment, globals);
  const missingIn: Record<Place, Set<string>> = { url: new Set(), headers: new Set(), auth: new Set(), body: new Set() };
  let at: Place = "url";
  const fill = (text: string) => {
    const done = preview(text, vars, secretAs);
    done.missing.forEach((m) => missingIn[at].add(m));
    return done.text;
  };
  const notes: string[] = [];
  let url = fill(request.url);
  at = "headers";
  const headers = request.headers.filter(({ key, disabled }) => !disabled && key !== "").map((h) => ({ key: fill(h.key), value: fill(h.value) }));

  at = "auth";

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
  if (auth.type === "digest") {
    fill(authParam(auth, "username"));
    fill(authParam(auth, "password"));
    if (!hasHeader(headers, "Authorization")) notes.push("digest auth answers the server's challenge, after a first request without it");
  }
  if (auth.type === "oauth2") {
    const token = authParam(auth, "accessToken");
    if (fetchesToken(auth)) {
      for (const key of ["accessTokenUrl", "refreshTokenUrl", "clientId", "clientSecret", "scope", "username", "password"]) fill(authParam(auth, key));
      notes.push("the OAuth 2.0 token is got or renewed as the request goes, and kept sealed");
    } else if (token !== "") {
      const filled = fill(token);
      if (authParam(auth, "addTokenTo") === "queryParams") url += `${url.includes("?") ? "&" : "?"}access_token=${filled}`;
      else if (!hasHeader(headers, "Authorization")) {
        const prefix = auth.params.some((p) => "headerPrefix" === p.key) ? authParam(auth, "headerPrefix") : "Bearer";
        headers.push({ key: "Authorization", value: prefix === "" ? filled : `${prefix} ${filled}` });
      }
    } else if (fetchingGrant(auth)) notes.push(`${authParam(auth, "grant_type")} needs a Token URL, or a token in Access token`);
    else if (authParam(auth, "grant_type") !== "") notes.push(`Kibo cannot get a token by ${authParam(auth, "grant_type")}: put one into Access token`);
  }
  if (auth.type === "other") notes.push(`${auth.name} auth is kept from Postman and not sent yet`);

  at = "body";
  let body: string | null = null;
  let form: FormField[] | null = null;
  if (request.body.mode === "raw") {
    // Comments go first, as Postman sends a JSON body: a variable in a part commented out is not missing.
    body = fill(request.body.language === "json" ? stripJsonComments(request.body.raw) : request.body.raw);
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
  if (request.body.mode === "formdata") {
    form = request.body.fields.filter(({ key, disabled }) => !disabled && key !== "").map((f) => (f.file === undefined ? { key: fill(f.key), value: fill(f.value) } : { key: fill(f.key), value: "", file: f.file }));
    body = form.map((f) => `${f.key}: ${f.file === undefined ? f.value : `<file ${f.file}>`}`).join("\n");
    if (form.some((f) => f.file)) notes.push("a file in a form is kept from Postman and not sent yet");
    if (!hasHeader(headers, "Content-Type")) headers.push({ key: "Content-Type", value: "multipart/form-data" });
  }
  if (request.body.mode === "other") notes.push(`a ${request.body.name} body is kept from Postman and not sent yet`);

  const by = (p: Place) => [...missingIn[p]];
  const all = new Set([...missingIn.url, ...missingIn.headers, ...missingIn.auth, ...missingIn.body]);
  return { method: request.method, url, headers, body, form, missing: [...all], missingIn: { url: by("url"), headers: by("headers"), auth: by("auth"), body: by("body") }, notes };
}

/** One shell word, in single quotes. */
const quote = (text: string) => `'${text.replace(/'/g, `'\\''`)}'`;

/** The request as a cURL command — a secret by its `{{name}}`, so the copy carries no token. */
export function toCurl(r: Resolved): string {
  const body = r.form === null && r.body !== null && r.body !== "";
  // curl's own method is GET, or POST with a body; HEAD is `-I`, as `-X HEAD` waits for a body that never comes.
  const method = r.method === "HEAD" ? " -I" : r.method === (body || r.form?.length ? "POST" : "GET") ? "" : ` -X ${r.method}`;
  const lines = [`curl${method} ${quote(r.url)}`];
  for (const h of r.headers) {
    // curl writes this one itself, with its boundary.
    if (r.form && /^content-type$/i.test(h.key)) continue;
    // Basic with a secret by its name is not encoded (base64 has no colon): curl encodes it once filled in.
    const basic = /^authorization$/i.test(h.key) ? /^Basic (.*:.*)$/.exec(h.value) : null;
    lines.push(basic ? `-u ${quote(basic[1])}` : `-H ${quote(`${h.key}: ${h.value}`)}`);
  }
  if (body) lines.push(`--data-raw ${quote(r.body!)}`);
  // `--form-string`: with `-F`, a value starting with `@` reads a file.
  for (const f of r.form ?? []) lines.push(f.file ? `-F ${quote(`${f.key}=@${f.file}`)}` : `--form-string ${quote(`${f.key}=${f.value}`)}`);
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
    .filter(({ key, value, disabled }) => !disabled && (key !== "" || value !== ""))
    .map((p) => (p.value === "" ? p.key : `${p.key}=${p.value}`))
    .join("&");
  return query ? `${base}?${query}` : base;
}

/** The Params tab's rows: the address's query, then the parameters turned off. */
export function paramsOf(request: HttpRequest): Pair[] {
  return [...queryOf(request.url), ...(request.paramsOff ?? []).map((p) => ({ ...p, disabled: true }))];
}

/** The Params tab writing back: a row on goes in the address, a row off beside it — so it can be turned on again. */
export function withParams(request: HttpRequest, rows: Pair[]): HttpRequest {
  const off = rows.filter((r) => r.disabled).map(({ key, value }) => ({ key, value }));
  return { ...request, url: withQuery(request.url, rows), paramsOff: off.length > 0 ? off : undefined };
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

export function findRequest(collections: Collection[], id: string): { collection: Collection; request: HttpRequest; path: string[]; ids: string[] } | null {
  for (const collection of collections) {
    // `path` names the collection and the folders down to the request, `ids` are theirs.
    const walk = (items: Item[], path: string[], ids: string[]): { request: HttpRequest; path: string[]; ids: string[] } | null => {
      for (const item of items) {
        if (item.kind === "request" && item.id === id) return { request: item, path, ids };
        if (item.kind === "folder") {
          const found = walk(item.items, [...path, item.name], [...ids, item.id]);
          if (found) return found;
        }
      }
      return null;
    };
    const found = walk(collection.items, [collection.name], [collection.id]);
    if (found) return { collection, ...found };
  }
  return null;
}

/** The items with the one called `id` changed, at any depth — or dropped, when `change` gives null. */
function changeItem(items: Item[], id: string, change: (item: Item) => Item | null): Item[] {
  return items.flatMap((item) => {
    if (item.id === id) {
      const next = change(item);
      return next ? [next] : [];
    }
    return item.kind === "folder" ? [{ ...item, items: changeItem(item.items, id, change) }] : [item];
  });
}

/** `request` put back in its place in the tree. */
export function replaceRequest(collections: Collection[], request: HttpRequest): Collection[] {
  return collections.map((c) => ({ ...c, items: changeItem(c.items, request.id, () => ({ kind: "request", ...request })) }));
}

/** A collection, folder or request under a new name. */
export function renameNode(collections: Collection[], id: string, name: string): Collection[] {
  return collections.map((c) => (c.id === id ? { ...c, name } : { ...c, items: changeItem(c.items, id, (item) => ({ ...item, name })) }));
}

/** A collection, folder or request gone, with everything under it. */
export function removeNode(collections: Collection[], id: string): Collection[] {
  return collections.filter((c) => c.id !== id).map((c) => ({ ...c, items: changeItem(c.items, id, () => null) }));
}

/** `request` added last in the collection or folder called `parent`. */
export function addRequest(collections: Collection[], parent: string, request: HttpRequest): Collection[] {
  return addItem(collections, parent, { kind: "request", ...request });
}

/** `item` — a request or a folder — added last in the collection or folder called `parent`. */
export function addItem(collections: Collection[], parent: string, item: Item): Collection[] {
  return collections.map((c) =>
    c.id === parent
      ? { ...c, items: [...c.items, item] }
      : { ...c, items: changeItem(c.items, parent, (folder) => (folder.kind === "folder" ? { ...folder, items: [...folder.items, item] } : folder)) },
  );
}

/** A folder or request, with all it holds, moved last into the collection or folder `to`. */
export function moveNode(collections: Collection[], id: string, to: string): Collection[] {
  const find = (items: Item[]): Item | null => {
    for (const item of items) {
      const found = item.id === id ? item : item.kind === "folder" ? find(item.items) : null;
      if (found) return found;
    }
    return null;
  };
  const moved = collections.map((c) => find(c.items)).find((item) => item) ?? null;
  return moved ? addItem(removeNode(collections, id), to, moved) : collections;
}

/** A collection or folder in the tree a move picks from; `here` is where the item is now, not a place to pick. */
export type MovePlace = { id: string; name: string; label: string; parents: string[]; here: boolean };

/**
 * The tree a folder or request may move into, in order: every collection and folder
 * but itself and what it holds, each with its path and the ones above it. A
 * collection whose file does not read is not in it.
 */
export function moveTargets(collections: Collection[], id: string): MovePlace[] {
  const out: MovePlace[] = [];
  const walk = (holder: { id: string; name: string; items: Item[] }, label: string, parents: string[]) => {
    out.push({ id: holder.id, name: holder.name, label, parents, here: holder.items.some((item) => item.id === id) });
    for (const item of holder.items) if (item.kind === "folder" && item.id !== id) walk(item, `${label} / ${item.name}`, [...parents, holder.id]);
  };
  for (const c of collections) if (!c.error) walk(c, c.name, []);
  return out;
}

/** The collection that holds `id` — a folder's, a request's, or its own. */
export function ownerOf(collections: Collection[], id: string): Collection | null {
  const holds = (items: Item[]): boolean => items.some((item) => item.id === id || (item.kind === "folder" && holds(item.items)));
  return collections.find((c) => c.id === id || holds(c.items)) ?? null;
}

/** The requests a collection, folder or request holds — what closes with it. */
export function requestsUnder(collections: Collection[], id: string): string[] {
  const all = (items: Item[]): string[] => items.flatMap((item) => (item.kind === "request" ? [item.id] : all(item.items)));
  const find = (items: Item[]): string[] | null => {
    for (const item of items) {
      if (item.id === id) return all([item]);
      const found = item.kind === "folder" ? find(item.items) : null;
      if (found) return found;
    }
    return null;
  };
  for (const c of collections) {
    const found = c.id === id ? all(c.items) : find(c.items);
    if (found) return found;
  }
  return [];
}

type Json = Record<string, unknown>;

const obj = (v: unknown): Json => (v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Json) : {});
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
/** A value as text, as the backend reads it: a string as it is, nothing as "", anything else as JSON. */
const text = (v: unknown): string => (typeof v === "string" ? v : v === undefined || v === null ? "" : JSON.stringify(v));
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

const pairsFrom = (v: unknown): Pair[] =>
  list(v)
    .map(obj)
    .map((p) => (p.disabled === true ? { key: text(p.key), value: text(p.value), disabled: true } : { key: text(p.key), value: text(p.value) }));

function authFrom(v: unknown): Auth {
  const auth = obj(v);
  const type = auth.type;
  // Absent or null, Postman's inherit.
  if (typeof type !== "string" || type === "inherit") return { type: "inherit" };
  const field = (key: string) => text(list(auth[type]).map(obj).find((p) => key === p.key)?.value);
  switch (type) {
    case "noauth":
      return { type };
    case "bearer":
      return { type, token: field("token") };
    case "basic":
      return { type, username: field("username"), password: field("password") };
    case "apikey":
      return { type, key: field("key"), value: field("value"), in: field("in") === "query" ? "query" : "header" };
    case "digest":
    case "oauth2":
      return { type, params: list(auth[type]).map(obj) };
    default:
      return { type: "other", name: type };
  }
}

function bodyFrom(v: unknown): Body {
  const body = obj(v);
  if (body.mode === "raw") {
    const language = text(obj(obj(body.options).raw).language);
    return { mode: "raw", language: language === "json" || language === "xml" ? language : "text", raw: text(body.raw) };
  }
  if (body.mode === "urlencoded") return { mode: "urlencoded", fields: pairsFrom(body.urlencoded) };
  if (body.mode === "formdata") {
    const path = (src: unknown) => (Array.isArray(src) ? src.map(text).join(", ") : text(src));
    const rows = list(body.formdata).map(obj);
    return { mode: "formdata", fields: pairsFrom(rows).map((pair, at) => (rows[at].type === "file" ? { ...pair, file: path(rows[at].src) } : pair)) };
  }
  return typeof body.mode === "string" ? { mode: "other", name: body.mode } : { mode: "none" };
}

function scriptsFrom(v: unknown): Scripts {
  const scripts: Scripts = {};
  for (const event of list(v).map(obj)) {
    const exec = obj(event.script).exec;
    if (event.listen === "prerequest" || event.listen === "test") scripts[event.listen] = Array.isArray(exec) ? exec.map(text).join("\n") : text(exec);
  }
  return scripts;
}

/** A Postman `request` — an object, or an address alone — as the page edits it. */
export function requestFrom(id: string, name: string, source: unknown, events?: unknown): HttpRequest {
  const request = typeof source === "string" ? { url: source } : obj(source);
  const url = request.url;
  // Postman keeps a parameter turned off in `query`, out of `raw`.
  const off = pairsFrom(obj(url).query)
    .filter((q) => q.disabled)
    .map(({ key, value }) => ({ key, value }));
  return {
    id,
    name,
    method: (text(request.method) || "GET").toUpperCase(),
    url: typeof url === "string" ? url : text(obj(url).raw),
    ...(off.length > 0 ? { paramsOff: off } : {}),
    headers: pairsFrom(request.header),
    auth: authFrom(request.auth),
    body: bodyFrom(request.body),
    scripts: scriptsFrom(events),
    source,
  };
}

function itemsFrom(v: unknown): Item[] {
  return list(v)
    .map(obj)
    .map((item): Item => {
      const id = text(item.id);
      const name = text(item.name);
      if (Array.isArray(item.item)) return { kind: "folder", id, name, auth: authFrom(item.auth), items: itemsFrom(item.item) };
      return { kind: "request", ...requestFrom(id, name, item.request, item.event) };
    });
}

/** A stored collection as the page draws it. */
export function collectionFrom(view: { id: string; file: unknown; error: string | null; current?: EnvVariable[] }): Collection {
  const file = obj(view.file);
  // Over the file's, for Preview only: the page never writes `variables` back.
  const current = (view.current ?? []).map(({ key, value, secret }) => (secret ? { key, value, secret } : { key, value }));
  return {
    id: view.id,
    name: view.error === null ? text(obj(file.info).name) : view.id,
    auth: authFrom(file.auth),
    // A `secret` one masked in Preview and named in cURL, as the backend masks it in history.
    variables: [
      ...pairsFrom(file.variable)
        .map((v, i) => {
          const row = obj(list(file.variable)[i]);
          return row.type === "secret" || row.secret === true ? { ...v, secret: true } : v;
        })
        .filter((v) => !current.some(({ key }) => key === v.key)),
      ...current,
    ],
    items: itemsFrom(file.item),
    scripts: scriptsFrom(file.event),
    ...(view.error === null ? {} : { error: view.error }),
  };
}

/** A row over the one it came from — its `type`, its `description` — when it is still that key. */
function pairOver(pair: Pair, old: unknown): Json {
  const out: Json = { ...obj(old), key: pair.key, value: pair.value };
  if (pair.disabled) out.disabled = true;
  else delete out.disabled;
  return out;
}

/** Rows over the ones they came from, matched by key in order, not by place: one deleted does not hand its neighbour's description on. */
function pairsOver(rows: Pair[], old: unknown): Json[] {
  const left = list(old).map(obj);
  return rows.map((row) => {
    const at = left.findIndex((was) => Object.is(was.key, row.key));
    return pairOver(row, at < 0 ? undefined : left.splice(at, 1)[0]);
  });
}

/** A file row stays one when renamed: matched by key, it would come back as empty text. */
const fileRow = (fields: FormField[]) => (row: Json, at: number): Json => {
  const file = fields[at].file;
  if (file === undefined) return { ...row, type: row.type ?? "text" };
  return { ...row, type: "file", ...(row.src === undefined && file !== "" ? { src: file } : {}) };
};

function authToPostman(auth: Auth): Json | undefined {
  const strings = (rows: [string, string][]) => rows.map(([key, value]) => ({ key, value, type: "string" }));
  switch (auth.type) {
    case "inherit":
      return undefined;
    case "noauth":
      return { type: "noauth" };
    case "bearer":
      return { type: "bearer", bearer: strings([["token", auth.token]]) };
    case "basic":
      return { type: "basic", basic: strings([["username", auth.username], ["password", auth.password]]) };
    case "apikey":
      return { type: "apikey", apikey: strings([["key", auth.key], ["value", auth.value], ["in", auth.in]]) };
    case "digest":
    case "oauth2":
      return { type: auth.type, [auth.type]: auth.params };
    case "other":
      return { type: auth.name };
  }
}

function bodyToPostman(body: Body, old: unknown): unknown {
  const was = obj(old);
  switch (body.mode) {
    case "none":
      return undefined;
    case "other":
      return old;
    case "raw": {
      const options = obj(was.options);
      // `html`, `javascript`: read as text, and kept as they were while text is what it says.
      const had = text(obj(options.raw).language);
      const language = body.language === "text" && had !== "" && had !== "json" && had !== "xml" ? had : body.language;
      return { ...(was.mode === "raw" ? was : {}), mode: "raw", raw: body.raw, options: { ...options, raw: { ...obj(options.raw), language } } };
    }
    case "urlencoded":
      return { ...(was.mode === "urlencoded" ? was : {}), mode: "urlencoded", urlencoded: pairsOver(body.fields, was.urlencoded) };
    case "formdata":
      return { ...(was.mode === "formdata" ? was : {}), mode: "formdata", formdata: pairsOver(body.fields, was.formdata).map(fileRow(body.fields)) };
  }
}

/**
 * An address as Postman writes it. One that was text stays text while no
 * parameter is off; otherwise its parts, as Postman's own export has them —
 * `raw` is what Kibo reads, the parts what other tools read — and the path
 * variables it had.
 */
function urlToPostman(raw: string, off: Pair[], old: unknown): unknown {
  if ((typeof old === "string" || old === undefined) && off.length === 0) return raw;
  const [, protocol, authority, path] = /^(?:([^:/?#]+):\/\/)?([^/?#]*)([^?#]*)/.exec(raw) ?? [];
  const port = /:(\d+)$/.exec(authority ?? "");
  const host = port ? authority.slice(0, port.index) : authority;
  const query = pairsOver([...queryOf(raw), ...off.map((p) => ({ ...p, disabled: true }))], obj(old).query);
  // The parts are made again from `raw`; the rest — `variable`, `hash`, a description — stays.
  const out: Json = { ...obj(old), raw };
  const put = (key: string, value: unknown) => (value === undefined ? delete out[key] : (out[key] = value));
  put("protocol", protocol || undefined);
  put("host", host ? host.split(".") : undefined);
  put("port", port ? port[1] : undefined);
  put("path", path ? path.replace(/^\//, "").split("/") : undefined);
  put("query", query.length > 0 ? query : undefined);
  return out;
}

/**
 * The request as Postman's `request` object: what the page changed written over
 * what it came as, and nothing else — an unedited request goes back as it came,
 * a header keeps its `description`, a body its options.
 */
export function requestToPostman(request: HttpRequest): unknown {
  const before = request.source === undefined ? null : requestFrom(request.id, request.name, request.source);
  const changed = (...keys: (keyof HttpRequest)[]) => before === null || keys.some((k) => !same(before[k], request[k]));
  if (!changed("method", "url", "paramsOff", "headers", "auth", "body")) return request.source;
  const was = typeof request.source === "string" ? { url: request.source } : obj(request.source);
  const out: Json = { ...was };
  if (changed("method") || !("method" in was)) out.method = request.method;
  if (changed("url", "paramsOff")) out.url = urlToPostman(request.url, request.paramsOff ?? [], was.url);
  if (changed("headers")) out.header = pairsOver(request.headers, was.header);
  if (changed("auth")) {
    const auth = authToPostman(request.auth);
    if (auth) out.auth = auth;
    else delete out.auth;
  }
  if (changed("body")) {
    const body = bodyToPostman(request.body, was.body);
    if (body) out.body = body;
    else delete out.body;
  }
  return out;
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

/** A body by its type: JSON and XML re-indented, with every digit and comment kept; anything else as it came. */
export function pretty(body: string, contentType: string): string {
  if (/json/i.test(contentType)) return formatJson(body);
  if (/xml/i.test(contentType)) return formatXml(body);
  return body;
}
