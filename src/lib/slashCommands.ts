import type { CommandFile, McpPromptItem } from "./chat";

/**
 * A command typed into the composer as `/name` or `/name arguments`.
 *
 * The composer knows only this shape — where a command comes from is not its
 * business. The built-in ones are made in `App.tsx`; the user's own are files
 * in `.kibo/commands`, turned into entries by `fileCommands`.
 */
export type SlashCommand = {
  name: string;
  /** What it does, beside its name in the menu. */
  hint: string;
  /** What to type after the name, shown beside it: `<file>`. */
  argumentHint?: string;
  /** Why it cannot run now, shown in its place; absent when it can. */
  unavailable?: string;
  run: (args: string) => void;
};

const NAME = "[a-z][\\w:-]*";

/**
 * `/name rest` → the name and what follows it. `null` when the text is not
 * shaped like a command — which is not the same as naming one that exists:
 * see `commandFor`.
 */
export function parseCommand(text: string): { name: string; args: string } | null {
  const m = new RegExp(`^\\/(${NAME})(?:\\s+([\\s\\S]*))?$`, "i").exec(text.trim());
  return m ? { name: m[1].toLowerCase(), args: (m[2] ?? "").trim() } : null;
}

/**
 * The command the text runs, with its arguments. A name no command has is not
 * one — `/usr/bin is missing` is a message, and is sent as one.
 */
export function commandFor(
  commands: readonly SlashCommand[],
  text: string,
): { command: SlashCommand; args: string } | null {
  const parsed = parseCommand(text);
  const command = parsed && commands.find((c) => c.name === parsed.name);
  return command && parsed ? { command, args: parsed.args } : null;
}

/** The commands offered while only a name is being typed: `/` alone offers all. */
export function suggestCommands(commands: readonly SlashCommand[], text: string): SlashCommand[] {
  const m = new RegExp(`^\\/(${NAME})?$`, "i").exec(text);
  if (!m) return [];
  const typed = (m[1] ?? "").toLowerCase();
  return commands.filter((c) => c.name.startsWith(typed));
}

/**
 * A command file's prompt with what was typed after its name put in, as
 * Claude Code does it, so a command written for it works here unchanged:
 *
 * - `$ARGUMENTS` — everything typed, as typed;
 * - `$ARGUMENTS[N]` and `$N` — one argument, counted from 0; quotes group a
 *   value with spaces in it (`"hello world"`). With no argument at that
 *   position the placeholder stays as written;
 * - `$name` — the argument at the position of `name` in the file's
 *   `arguments`, empty when nothing was typed there;
 * - `\$1` — a literal `$1`. One backslash only: `\\$1` keeps both and still
 *   puts the argument in.
 *
 * One pass: an argument that itself says `$1` is put in as text. When no
 * placeholder took anything, what was typed is added at the end as
 * `ARGUMENTS: …` — typed and silently dropped is worse than put somewhere the
 * author did not plan for.
 */
export function expandTemplate(template: string, args: string, names: readonly string[] = []): string {
  const words = splitArguments(args);
  const named = names.map((n) => n.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const token = new RegExp(`(\\\\*)\\$(?:ARGUMENTS\\[(\\d+)\\]|(\\d+)|(${["ARGUMENTS", ...named].join("|")})(?!\\w))`, "g");
  let took = false;
  const expanded = template.replace(token, (whole: string, slashes: string, index?: string, digits?: string, word?: string) => {
    if (slashes.length === 1) return whole.slice(1);
    const value =
      word === "ARGUMENTS" ? args : word !== undefined ? (words[names.indexOf(word)] ?? "") : words[Number(index ?? digits)];
    if (value === undefined) return whole;
    took = true;
    return slashes + value;
  });
  return took || !args ? expanded : `${expanded}\n\nARGUMENTS: ${args}`;
}

/** What was typed, as a shell splits it: on spaces, with quotes grouping. */
function splitArguments(text: string): string[] {
  const words: string[] = [];
  let word: string | null = null;
  let quote: string | null = null;
  for (const c of text) {
    if (quote) {
      if (c === quote) quote = null;
      else word += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      word ??= "";
    } else if (/\s/.test(c)) {
      if (word !== null) words.push(word);
      word = null;
    } else {
      word = (word ?? "") + c;
    }
  }
  if (word !== null) words.push(word);
  return words;
}

/** A command as the transcript shows it: what was typed, not the prompt it sent. */
export function typedCommand(name: string, args: string): string {
  return args ? `/${name} ${args}` : `/${name}`;
}

/**
 * The user's command files as menu entries, each sending its prompt — shown
 * in the transcript as the command typed. A file named like a built-in is
 * left out: `/compact` stays the app's.
 */
export function fileCommands(
  files: readonly CommandFile[],
  builtIn: readonly SlashCommand[],
  send: (text: string, sent: string) => void,
): SlashCommand[] {
  return files
    .filter((file) => !builtIn.some((c) => c.name === file.name))
    .map((file) => ({
      name: file.name,
      hint: file.description,
      argumentHint: file.argumentHint ?? undefined,
      run: (args: string) => send(typedCommand(file.name, args), expandTemplate(file.template, args, file.arguments)),
    }));
}

/**
 * The running MCP servers' prompts as menu entries, `/<server>:<prompt>`. The
 * server writes the prompt when it runs, with what was typed after the name
 * given to its arguments; the transcript shows the command typed, as for a
 * command file. A name already taken — built in, or a file — stays the
 * other's.
 */
export function mcpPromptCommands(
  prompts: readonly McpPromptItem[],
  taken: readonly SlashCommand[],
  write: (server: string, prompt: string, args: string) => Promise<string>,
  send: (text: string, sent: string) => void,
  fail: (why: string) => void,
): SlashCommand[] {
  return prompts
    .map((prompt) => ({ prompt, name: `${prompt.server}:${prompt.name}`.toLowerCase() }))
    .filter(({ name }) => !taken.some((c) => c.name === name))
    .map(({ prompt, name }) => ({
      name,
      hint: `${prompt.title || prompt.description || prompt.name} — from ${prompt.server}`,
      argumentHint: prompt.arguments.map((a) => (a.required ? `<${a.name}>` : `[${a.name}]`)).join(" ") || undefined,
      run: (args: string) => {
        write(prompt.server, prompt.name, args).then(
          (text) => send(typedCommand(name, args), text),
          (e) => fail(`${prompt.server} could not write /${name}: ${e}`),
        );
      },
    }));
}
