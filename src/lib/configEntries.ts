// One entry of the MCP or hooks file — a server, a hook command — read into
// form fields and written back. The file stays the truth: an edit rewrites
// the text and saves it the way the JSON editor does, and whatever the form
// has no field for (`disabled`, Claude Code's extra keys) is carried over
// untouched.

import type { McpExposure } from "./mcp";
import { isObject, jsonError, parseFile, parseLoose, snippetServers, stringify, type Json } from "./configSnippets";

export type Written = { text: string } | { error: string };

/** A whole number of at least 1, or empty for "the default". */
function positive(value: string, field: string): number | undefined | { error: string } {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const n = Number(trimmed);
  return Number.isInteger(n) && n >= 1 ? n : { error: `${field} must be a whole number of at least 1` };
}

// ─── MCP servers ───────────────────────────────────────────────────────────

/**
 * Args one per line and env as KEY=VALUE lines: what a README's snippet
 * splits into. A server either runs a command or is reached at a URL; the
 * fields of the other kind are ignored on save.
 */
export type McpServerFields = {
  name: string;
  transport: "command" | "url";
  command: string;
  args: string;
  env: string;
  url: string;
  /** `Name: value`, one per line. */
  headers: string;
  weight: string;
  timeoutSecs: string;
  /** How the model reaches its tools; `direct`, the default, is written as nothing. */
  exposure: McpExposure;
  /** Its results are never cleared to save context; `false`, the default, is written as nothing. */
  keepResults: boolean;
  /** Everything the form has no field for — `disabled`, `type`, another client's keys — carried as it came. */
  extra: Json;
};

const EXPOSURES: McpExposure[] = ["direct", "deferred", "hidden"];

export const EMPTY_SERVER: McpServerFields = {
  name: "",
  transport: "command",
  command: "",
  args: "",
  env: "",
  url: "",
  headers: "",
  weight: "",
  timeoutSecs: "",
  exposure: "direct",
  keepResults: false,
  extra: {},
};

const pairs = (value: unknown, separator: string) =>
  Object.entries(isObject(value) ? value : {})
    .map(([key, v]) => `${key}${separator}${String(v)}`)
    .join("\n");

/** A key of its own, not one every object inherits: a server may be called `constructor`. */
const own = (servers: Json, name: string) => Object.prototype.hasOwnProperty.call(servers, name);

/**
 * The entry a write takes the place of: the one being edited, or the one the
 * typed name already names — a pasted server whose name is in the file replaces
 * it where it stands, as the file editor's own paste does.
 */
export function previousEntry(text: string, edited: string | null, typed: string): string | null {
  if (edited !== null) return edited;
  const servers = parseFile(text)?.mcpServers;
  return isObject(servers) && typed && own(servers, typed) ? typed : null;
}

/** The server's part of a tool name, as `domain::mcp::server_key` forms it: `mcp__<key>__<tool>`. */
export function serverKey(name: string): string {
  return Array.from(name, (c) => (/[A-Za-z0-9-]/.test(c) ? c.toLowerCase() : "_")).join("");
}

/**
 * Another server in the file whose tools would take the same prefix, if there
 * is one: a name is lowercased and what a tool name may not hold becomes `_`,
 * so two spellings of one server become one prefix there.
 */
export function keyClash(text: string, previous: string | null, typed: string): string | null {
  const key = serverKey(typed);
  if (!key) return null;
  const servers = parseFile(text)?.mcpServers;
  if (!isObject(servers)) return null;
  return Object.keys(servers).find((other) => other !== previous && serverKey(other) === key) ?? null;
}

/** `${NAME}` as the catalog reads one: a letter or `_`, then letters, digits or `_`. */
const PLACEHOLDER = /\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g;

/** A block's values in the order the backend reads them — its maps are sorted by key. */
const valuesOf = (block: unknown) =>
  isObject(block) ? Object.keys(block).sort().map((key) => String(block[key])) : [];

const entryIn = (text: string, name: string): Json | null => {
  const servers = parseFile(text)?.mcpServers;
  const entry = isObject(servers) && own(servers, name) ? servers[name] : undefined;
  return isObject(entry) ? entry : null;
};

/**
 * The `${NAME}` placeholders in an entry's `env` and `headers` values, each
 * once, in the order they come — the catalog's convention, which fills them in
 * at install: nothing reads them when a server starts.
 */
export function entrySecrets(text: string, name: string): string[] {
  const entry = entryIn(text, name);
  if (!entry) return [];
  const names: string[] = [];
  for (const value of [...valuesOf(entry.env), ...valuesOf(entry.headers)]) {
    for (const match of value.matchAll(PLACEHOLDER)) if (!names.includes(match[1])) names.push(match[1]);
  }
  return names;
}

/**
 * The file with every `${NAME}` in the entry's `env` and `headers` replaced by
 * its value, trimmed — a token pasted with its newline is the token. In one
 * pass, so a value that itself holds `${…}` stays as typed. Refused while one
 * is empty, or has a control character: a line break in a header is another
 * header.
 */
export function fillSecrets(text: string, name: string, secrets: Record<string, string>): Written {
  const file = parseFile(text);
  const entry = entryIn(text, name);
  if (!file || !entry) return { error: `The file has no server called ${name}` };
  let problem: string | null = null;
  const valueOf = (secret: string) => {
    const value = secrets[secret]?.trim() ?? "";
    if (!value) problem ??= `${secret} is not filled in`;
    else if (/\p{Cc}/u.test(value)) problem ??= `${secret} has a line break or another control character in it — paste the value alone`;
    return value;
  };
  const fill = (block: unknown) =>
    isObject(block)
      ? Object.fromEntries(
          Object.entries(block).map(([key, value]) => [key, typeof value === "string" ? value.replace(PLACEHOLDER, (_, secret: string) => valueOf(secret)) : value]),
        )
      : block;
  const filled: Json = { ...entry, env: fill(entry.env), headers: fill(entry.headers) };
  const written = stringify({ ...file, mcpServers: { ...(file.mcpServers as Json), [name]: filled } });
  return problem ? { error: problem } : { text: written };
}

/** The keys the form writes itself; anything else an entry carries stays in `extra`. */
const OWNED = ["command", "args", "env", "url", "headers", "weight", "timeoutSecs", "exposure", "keepResults"] as const;

/** One entry of `mcpServers` as the form's fields, whichever way it was read. */
export function serverFields(name: string, server: Json): McpServerFields {
  const url = typeof server.url === "string" ? server.url : "";
  return {
    name,
    transport: url ? "url" : "command",
    command: typeof server.command === "string" ? server.command : "",
    args: Array.isArray(server.args) ? server.args.map(String).join("\n") : "",
    env: pairs(server.env, "="),
    url,
    headers: pairs(server.headers, ": "),
    weight: server.weight === undefined ? "" : String(server.weight),
    timeoutSecs: server.timeoutSecs === undefined ? "" : String(server.timeoutSecs),
    exposure: EXPOSURES.find((e) => e === server.exposure) ?? "direct",
    keepResults: server.keepResults === true,
    extra: Object.fromEntries(Object.entries(server).filter(([key]) => !(OWNED as readonly string[]).includes(key))),
  };
}

export function readMcpServer(text: string, name: string): McpServerFields | null {
  const servers = parseFile(text)?.mcpServers;
  const server = isObject(servers) ? servers[name] : undefined;
  return isObject(server) ? serverFields(name, server) : null;
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

/** `KEY<separator>VALUE` lines as an object, or which line is not one. */
function keyed(text: string, separator: string): Json | { bad: string } {
  const out: Json = {};
  for (const line of lines(text)) {
    const at = line.indexOf(separator);
    if (at < 1) return { bad: line };
    out[line.slice(0, at).trim()] = line.slice(at + 1).trim();
  }
  return out;
}

/**
 * How the server runs, as the entry's keys. A whole command line typed into
 * Command (`npx -y server-github`) is split into command and args when no
 * args were given; one with quotes is left alone, since splitting it would
 * need a shell.
 */
function transport(fields: McpServerFields): Json | { error: string } {
  if (fields.transport === "url") {
    const url = fields.url.trim();
    if (!url) return { error: "The server needs a URL" };
    const headers = keyed(fields.headers, ":");
    if ("bad" in headers) return { error: `Header lines are Name: value — "${headers.bad}" is not` };
    // `type` for Claude Code, which reads an entry without it as a command.
    return { type: "http", url, ...(Object.keys(headers).length ? { headers } : {}) };
  }
  let command = fields.command.trim();
  let args = lines(fields.args);
  if (!command) return { error: "The server needs a command to start it" };
  if (args.length === 0 && /\s/.test(command) && !/["']/.test(command)) {
    [command, ...args] = command.split(/\s+/);
  }
  const env = keyed(fields.env, "=");
  if ("bad" in env) return { error: `Environment lines are KEY=VALUE — "${env.bad}" is not` };
  return { command, ...(args.length ? { args } : {}), ...(Object.keys(env).length ? { env } : {}) };
}

/**
 * The server written under its name — in place, so the file keeps its order,
 * or at the end when it is new. What the entry said about the other kind of
 * server goes: a URL and a command together would not start.
 */
export function writeMcpServer(text: string, previous: string | null, fields: McpServerFields): Written {
  const file = parseFile(text);
  if (!file) return { error: "The file is not valid JSON — fix it in the JSON editor first" };
  const name = fields.name.trim();
  if (!name) return { error: "The server needs a name" };
  const servers = isObject(file.mcpServers) ? file.mcpServers : {};
  if (name !== previous && own(servers, name)) return { error: `There is already a server called ${name}` };

  const how = transport(fields);
  if ("error" in how) return how as { error: string };
  const weight = positive(fields.weight, "Weight");
  const timeoutSecs = positive(fields.timeoutSecs, "Timeout");
  for (const n of [weight, timeoutSecs]) if (isObject(n)) return n as { error: string };

  // What the form has no field for comes from the file it edits, or — for an
  // entry just pasted in — from the snippet.
  const kept = previous !== null && own(servers, previous) && isObject(servers[previous]) ? { ...servers[previous], ...fields.extra } : fields.extra;
  const { command: _c, args: _a, env: _e, url: _u, headers: _h, weight: _w, timeoutSecs: _t, exposure: _x, keepResults: _k, ...rest } = kept;
  // A `type` saying the entry is at a URL would contradict a command; one at
  // a URL gets its own `type` from `transport`.
  if (rest.type === "http" || rest.type === "sse") delete rest.type;
  const server: Json = {
    ...rest,
    ...how,
    ...(weight !== undefined ? { weight } : {}),
    ...(timeoutSecs !== undefined ? { timeoutSecs } : {}),
    ...(fields.exposure !== "direct" ? { exposure: fields.exposure } : {}),
    ...(fields.keepResults ? { keepResults: true } : {}),
  };
  const entries = Object.entries(servers);
  const at = previous === null ? -1 : entries.findIndex(([key]) => key === previous);
  if (at < 0) entries.push([name, server]);
  else entries[at] = [name, server];
  return { text: stringify({ ...file, mcpServers: Object.fromEntries(entries) }) };
}

/** A server read from a pasted snippet, before anything is written. */
export type PastedServer = { name: string; fields: McpServerFields };

/**
 * The servers a pasted snippet holds, as the fields the form edits. The snippet
 * is the one the whole-file editor takes (`parseLoose`): a whole
 * `{"mcpServers": {…}}`, a bare `{"github": {…}}`, or `"github": {…},` cut out
 * of one. An entry with no name of its own comes back as `""`, which the form
 * then asks for. `exposure` is the screen's default for a snippet that says
 * nothing about it — a README rarely does.
 */
export function pastedServers(
  text: string,
  exposure?: McpExposure,
): { servers: PastedServer[] } | { error: string } {
  if (!text.trim()) return { error: "Paste a server's JSON first" };
  const snippet = parseLoose(text);
  if (snippet === undefined) {
    const problem = jsonError(text);
    const at = problem?.line ? `Line ${problem.line}, column ${problem.column}: ` : "";
    return { error: `${at}${problem?.message ?? "Not valid JSON"}` };
  }
  const found = snippetServers(snippet);
  if (!found) return { error: "Not an MCP server: its entry needs a command or a url" };
  const entries = Object.entries(found.servers);
  if (entries.length === 0) return { error: "Nothing to read: the snippet holds no servers" };
  const servers: PastedServer[] = [];
  for (const [name, server] of entries) {
    if (!isObject(server)) return { error: `Not an MCP server: ${name || "the entry"} is not an object` };
    // A value the fields cannot hold would be dropped on the way in, so the
    // paste is refused instead: `args` one per line, `env` and `headers` as
    // objects are the shapes the entry is written in.
    if (server.args !== undefined && !Array.isArray(server.args)) return { error: `${name || "The entry"}: args must be a list` };
    if (server.env !== undefined && !isObject(server.env)) return { error: `${name || "The entry"}: env must be an object` };
    if (server.headers !== undefined && !isObject(server.headers)) return { error: `${name || "The entry"}: headers must be an object` };
    servers.push({ name, fields: exposure && server.exposure === undefined ? { ...serverFields(name, server), exposure } : serverFields(name, server) });
  }
  return { servers };
}

export function removeMcpServer(text: string, name: string): string | null {
  const file = parseFile(text);
  if (!file || !isObject(file.mcpServers) || !own(file.mcpServers, name)) return null;
  const { [name]: _gone, ...servers } = file.mcpServers;
  return stringify({ ...file, mcpServers: servers });
}

// ─── Hooks ─────────────────────────────────────────────────────────────────

export type HookFields = { event: string; matcher: string; command: string; timeout: string };

export const EMPTY_HOOK: HookFields = { event: "PreToolUse", matcher: "", command: "", timeout: "" };

type Place = { event: string; group: number; command: number };

/**
 * The file's hook commands in the order the Hooks tab lists them: events by
 * name — the backend keeps them in a sorted map — then groups and commands as
 * written. Row `i` of the tab is `places(file)[i]`.
 */
function places(hooks: Json): Place[] {
  return Object.keys(hooks)
    .sort()
    .flatMap((event) => {
      const groups = hooks[event];
      if (!Array.isArray(groups)) return [];
      return groups.flatMap((group, g) =>
        isObject(group) && Array.isArray(group.hooks) ? group.hooks.map((_, c) => ({ event, group: g, command: c })) : [],
      );
    });
}

type Group = { matcher?: string; hooks?: Json[] } & Json;
const groupsOf = (hooks: Json, event: string) => (Array.isArray(hooks[event]) ? (hooks[event] as Group[]) : []);
// A group may have no `hooks` at all — the backend reads that as none.
const commandsOf = (group: Group) => (Array.isArray(group.hooks) ? group.hooks : []);
const commandAt = (hooks: Json, place: Place) => commandsOf(groupsOf(hooks, place.event)[place.group])[place.command];

export function readHook(text: string, index: number): HookFields | null {
  const hooks = parseFile(text)?.hooks;
  if (!isObject(hooks)) return null;
  const place = places(hooks)[index];
  if (!place) return null;
  const group = groupsOf(hooks, place.event)[place.group];
  const hook = commandAt(hooks, place);
  return {
    event: place.event,
    matcher: typeof group.matcher === "string" ? group.matcher : "",
    command: isObject(hook) && typeof hook.command === "string" ? hook.command : "",
    timeout: isObject(hook) && hook.timeout !== undefined ? String(hook.timeout) : "",
  };
}

/** The hooks with one command taken out, and its group and event too if that leaves them empty. */
function without(hooks: Json, place: Place): Json {
  const groups = groupsOf(hooks, place.event)
    .map((group, g) => (g === place.group ? { ...group, hooks: commandsOf(group).filter((_, c) => c !== place.command) } : group))
    .filter((group, g) => g !== place.group || commandsOf(group).length > 0);
  const { [place.event]: _gone, ...rest } = hooks;
  return groups.length ? { ...hooks, [place.event]: groups } : rest;
}

/**
 * The hook written back. An edit that keeps its event and matcher stays where
 * it was; otherwise it moves to the group under its event with the same
 * matcher, or a new one at the end — the matcher belongs to the group, not to
 * the command.
 */
export function writeHook(text: string, index: number | null, fields: HookFields): Written {
  const file = parseFile(text);
  if (!file) return { error: "The file is not valid JSON — fix it in the JSON editor first" };
  const command = fields.command.trim();
  if (!command) return { error: "The hook needs a command to run" };
  const timeout = positive(fields.timeout, "Timeout");
  if (isObject(timeout)) return timeout as { error: string };
  const matcher = fields.event === "Stop" ? "" : fields.matcher.trim();

  let hooks = isObject(file.hooks) ? file.hooks : {};
  const place = index === null ? undefined : places(hooks)[index];
  if (index !== null && !place) return { error: "This hook is no longer in the file — it was changed meanwhile" };

  const old = place ? commandAt(hooks, place) : undefined;
  const { timeout: _t, ...kept } = isObject(old) ? old : { type: "command" };
  const entry: Json = { ...kept, command, ...(timeout !== undefined ? { timeout } : {}) };

  if (place) {
    const group = groupsOf(hooks, place.event)[place.group];
    if (place.event === fields.event && (group.matcher ?? "").trim() === matcher) {
      const groups = groupsOf(hooks, place.event).map((g, i) =>
        i === place.group ? { ...g, hooks: commandsOf(g).map((h, c) => (c === place.command ? entry : h)) } : g,
      );
      return { text: stringify({ ...file, hooks: { ...hooks, [place.event]: groups } }) };
    }
    hooks = without(hooks, place);
  }

  const groups = groupsOf(hooks, fields.event);
  const same = groups.findIndex((g) => (g.matcher ?? "").trim() === matcher);
  const next =
    same >= 0
      ? groups.map((g, i) => (i === same ? { ...g, hooks: [...commandsOf(g), entry] } : g))
      : [...groups, { ...(matcher ? { matcher } : {}), hooks: [entry] }];
  return { text: stringify({ ...file, hooks: { ...hooks, [fields.event]: next } }) };
}

export function removeHook(text: string, index: number): string | null {
  const file = parseFile(text);
  if (!file || !isObject(file.hooks)) return null;
  const place = places(file.hooks)[index];
  return place ? stringify({ ...file, hooks: without(file.hooks, place) }) : null;
}
