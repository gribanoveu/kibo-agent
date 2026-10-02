// One entry of the MCP or hooks file — a server, a hook command — read into
// form fields and written back. The file stays the truth: an edit rewrites
// the text and saves it the way the JSON editor does, and whatever the form
// has no field for (`disabled`, Claude Code's extra keys) is carried over
// untouched.

import type { McpExposure } from "./chat";
import { isObject, parseFile, stringify, type Json } from "./configSnippets";

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
};

const pairs = (value: unknown, separator: string) =>
  Object.entries(isObject(value) ? value : {})
    .map(([key, v]) => `${key}${separator}${String(v)}`)
    .join("\n");

export function readMcpServer(text: string, name: string): McpServerFields | null {
  const servers = parseFile(text)?.mcpServers;
  const server = isObject(servers) ? servers[name] : undefined;
  if (!isObject(server)) return null;
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
  };
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
  if (name !== previous && name in servers) return { error: `There is already a server called ${name}` };

  const how = transport(fields);
  if ("error" in how) return how as { error: string };
  const weight = positive(fields.weight, "Weight");
  const timeoutSecs = positive(fields.timeoutSecs, "Timeout");
  for (const n of [weight, timeoutSecs]) if (isObject(n)) return n as { error: string };

  const kept = previous !== null && isObject(servers[previous]) ? servers[previous] : {};
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

export function removeMcpServer(text: string, name: string): string | null {
  const file = parseFile(text);
  if (!file || !isObject(file.mcpServers) || !(name in file.mcpServers)) return null;
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
