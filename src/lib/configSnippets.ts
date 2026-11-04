// The MCP and hooks files are edited as JSON, in one box. What makes that box
// bearable lives here: where the text stops being JSON, and how a snippet
// copied from a README joins the file instead of breaking it.

export type JsonProblem = { message: string; line?: number; column?: number };

/**
 * Why the text is not JSON, and where. `JSON.parse` decides; the scan below
 * only says where, since WebKit's messages carry no position at all.
 */
export function jsonError(text: string): JsonProblem | null {
  try {
    JSON.parse(text);
    return null;
  } catch (e) {
    const found = locate(text);
    if (!found) return { message: String(e instanceof Error ? e.message : e) };
    const before = text.slice(0, found.at).split("\n");
    return { message: found.message, line: before.length, column: before[before.length - 1].length + 1 };
  }
}

/** A strict JSON scan that stops at the first thing out of place. */
function locate(text: string): { at: number; message: string } | null {
  let i = 0;
  const fail = (message: string): never => {
    throw { at: i, message };
  };
  const ws = () => {
    while (i < text.length && " \t\n\r".includes(text[i])) i++;
  };
  const string = () => {
    i++;
    while (i < text.length) {
      const c = text[i];
      if (c === '"') return void i++;
      if (c === "\n") fail("The string is not closed on this line");
      i += c === "\\" ? 2 : 1;
    }
    fail("The string is not closed");
  };
  const NUMBER = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y;
  const members = (close: string, member: () => void) => {
    i++;
    ws();
    if (text[i] === close) return void i++;
    for (;;) {
      member();
      ws();
      if (text[i] === close) return void i++;
      if (text[i] !== ",") fail(`Expected "," or "${close}"`);
      i++;
      ws();
      if (text[i] === close) fail(`A comma before "${close}" is not allowed in JSON`);
    }
  };
  const value = (): void => {
    ws();
    const c = text[i];
    if (c === "{")
      return members("}", () => {
        ws();
        if (text[i] !== '"') fail("Expected a key in double quotes");
        string();
        ws();
        if (text[i] !== ":") fail('Expected ":" after the key');
        i++;
        value();
      });
    if (c === "[") return members("]", value);
    if (c === '"') return string();
    NUMBER.lastIndex = i;
    if (NUMBER.test(text)) return void (i = NUMBER.lastIndex);
    for (const word of ["true", "false", "null"]) if (text.startsWith(word, i)) return void (i += word.length);
    fail(i >= text.length ? "The text ends too early" : `Unexpected ${JSON.stringify(c)}`);
  };
  try {
    value();
    ws();
    if (i < text.length) fail("Unexpected text after the end");
    return null;
  } catch (e) {
    return e as { at: number; message: string };
  }
}

export type Json = Record<string, unknown>;
export const isObject = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);

/** A snippet as READMEs print it: a whole object, or just `"name": {…},` cut out of one. */
export function parseLoose(text: string): unknown {
  const trimmed = text.trim().replace(/,$/, "");
  for (const candidate of [trimmed, `{${trimmed}}`]) {
    try {
      return JSON.parse(candidate);
    } catch {
      // The next spelling, or not a snippet at all.
    }
  }
  return undefined;
}

/** The file being edited, if it is an object — the only shape a snippet can join. */
export function parseFile(text: string): Json | null {
  try {
    const file: unknown = JSON.parse(text.trim() || "{}");
    return isObject(file) ? file : null;
  } catch {
    return null;
  }
}

export const stringify = (file: Json) => `${JSON.stringify(file, null, 2)}\n`;

/**
 * The servers a pasted snippet holds, by name: `{"mcpServers": {…}}`, a bare
 * `{"github": {…}}`, or one entry with no name of its own. `unnamed` is that
 * last one — the file's paste has no place for it, and the install screen asks
 * for the name it lacks.
 */
export function snippetServers(snippet: unknown): { servers: Json; unnamed: boolean } | null {
  if (!isObject(snippet)) return null;
  if (isObject(snippet.mcpServers)) return { servers: snippet.mcpServers, unnamed: false };
  const values = Object.values(snippet);
  if (values.length > 0 && values.every((v) => isObject(v) && ("command" in v || "url" in v))) {
    return { servers: snippet, unnamed: false };
  }
  if ("command" in snippet || "url" in snippet) return { servers: { "": snippet }, unnamed: true };
  return null;
}

export type Merged = { text: string; message: string };

/**
 * A pasted MCP snippet added to the file: `{"mcpServers": {…}}`, a bare
 * `{"github": {…}}`, or `"github": {…}` alone. A server already there by that
 * name is replaced, and the message says so. Anything else is not a snippet.
 */
export function mergeMcp(current: string, pasted: string): Merged | null {
  const found = snippetServers(parseLoose(pasted));
  const file = parseFile(current);
  // An entry with no name of its own has no name to go under in the file.
  if (!found || found.unnamed || !file) return null;
  const { servers } = found;
  const names = Object.keys(servers);
  if (names.length === 0) return null;

  const existing = isObject(file.mcpServers) ? file.mcpServers : {};
  const replaced = names.filter((name) => name in existing);
  return {
    text: stringify({ ...file, mcpServers: { ...existing, ...servers } }),
    message:
      `Added ${names.join(", ")}` + (replaced.length ? ` — replaced the ${replaced.join(", ")} already here` : ""),
  };
}

/**
 * A pasted hooks snippet added to the file: `{"hooks": {…}}` or the events
 * alone. Groups join the ones already under the same event; one identical to
 * a group already there is not added twice.
 */
export function mergeHooks(current: string, pasted: string): Merged | null {
  const snippet = parseLoose(pasted);
  if (!isObject(snippet)) return null;
  const events = isObject(snippet.hooks) ? snippet.hooks : snippet;
  const entries = Object.entries(events);
  const isGroups = (v: unknown): v is unknown[] =>
    Array.isArray(v) && v.every((group) => isObject(group) && Array.isArray(group.hooks));
  const file = parseFile(current);
  if (entries.length === 0 || !entries.every(([, groups]) => isGroups(groups)) || !file) return null;

  const existing = isObject(file.hooks) ? file.hooks : {};
  const next: Json = { ...existing };
  for (const [event, groups] of entries as [string, unknown[]][]) {
    const had = Array.isArray(existing[event]) ? (existing[event] as unknown[]) : [];
    const seen = new Set(had.map((group) => JSON.stringify(group)));
    next[event] = [...had, ...groups.filter((group) => !seen.has(JSON.stringify(group)))];
  }
  return { text: stringify({ ...file, hooks: next }), message: `Added hooks for ${entries.map(([e]) => e).join(", ")}` };
}
