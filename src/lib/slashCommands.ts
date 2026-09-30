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
 * A command file's prompt with what was typed after its name put in, for
 * every `$ARGUMENTS`. A prompt without the placeholder still gets them, after
 * a blank line: arguments typed and silently dropped are worse than ones put
 * somewhere the author did not plan for.
 */
export function expandTemplate(template: string, args: string): string {
  if (template.includes("$ARGUMENTS")) return template.split("$ARGUMENTS").join(args);
  return args ? `${template}\n\n${args}` : template;
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
      run: (args: string) => send(typedCommand(file.name, args), expandTemplate(file.template, args)),
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
