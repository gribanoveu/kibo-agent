import { describe, expect, test } from "bun:test";
import {
  commandFor,
  expandTemplate,
  fileCommands,
  mcpPromptCommands,
  parseCommand,
  suggestCommands,
  type SlashCommand,
} from "../lib/slashCommands";
import type { CommandFile } from "../lib/chat";

const command = (name: string): SlashCommand => ({ name, hint: "", run: () => {} });
const commands = [command("compact"), command("fork")];

describe("a typed command", () => {
  test("is its name and what follows it", () => {
    expect(parseCommand("/fork")).toEqual({ name: "fork", args: "" });
    expect(parseCommand("  /Compact keep the plan \n")).toEqual({ name: "compact", args: "keep the plan" });
    expect(parseCommand("/review src/a.ts\nand b")).toEqual({ name: "review", args: "src/a.ts\nand b" });
  });

  test("is not text that merely starts with a slash", () => {
    expect(parseCommand("/")).toBeNull();
    expect(parseCommand("fork")).toBeNull();
    expect(parseCommand("/usr/bin is missing")).toBeNull();
    expect(parseCommand("// comment")).toBeNull();
  });

  test("runs only when a command has that name", () => {
    expect(commandFor(commands, "/fork now")?.command.name).toBe("fork");
    expect(commandFor(commands, "/fork now")?.args).toBe("now");
    expect(commandFor(commands, "/tmp is full")).toBeNull();
    expect(commandFor(commands, "/for")).toBeNull();
  });
});

describe("the offered commands", () => {
  test("are those starting with what is typed, all of them for a bare slash", () => {
    expect(suggestCommands(commands, "/").map((c) => c.name)).toEqual(["compact", "fork"]);
    expect(suggestCommands(commands, "/F").map((c) => c.name)).toEqual(["fork"]);
    expect(suggestCommands(commands, "/fork").map((c) => c.name)).toEqual(["fork"]);
    expect(suggestCommands(commands, "/x")).toEqual([]);
  });

  test("are gone once arguments start, and for anything else", () => {
    expect(suggestCommands(commands, "/fork ")).toEqual([]);
    expect(suggestCommands(commands, " /fork")).toEqual([]);
    expect(suggestCommands(commands, "hello")).toEqual([]);
    expect(suggestCommands(commands, "")).toEqual([]);
  });
});

describe("a command file", () => {
  test("puts what was typed where the prompt says", () => {
    expect(expandTemplate("Review $ARGUMENTS, then $ARGUMENTS again", "src/a.ts")).toBe(
      "Review src/a.ts, then src/a.ts again",
    );
    expect(expandTemplate("Review $ARGUMENTS", "")).toBe("Review ");
  });

  /// Typed and dropped would be worse than typed and put at the end — as
  /// Claude Code puts it.
  test("without the placeholder, keeps what was typed after the prompt", () => {
    expect(expandTemplate("Fix the build", "only the linux job")).toBe("Fix the build\n\nARGUMENTS: only the linux job");
    expect(expandTemplate("Fix the build", "")).toBe("Fix the build");
  });

  /// Claude Code's numbering: from 0, so its commands mean the same here.
  test("puts one argument where $N or $ARGUMENTS[N] says, counting from 0", () => {
    const migrate = "Migrate $0 from $1 to $ARGUMENTS[2].";
    expect(expandTemplate(migrate, "SearchBar JavaScript TypeScript")).toBe("Migrate SearchBar from JavaScript to TypeScript.");
    expect(expandTemplate("$0|$1", `"hello world" 'it''s'  second`)).toBe("hello world|its");
    expect(expandTemplate("$1", `a "" c`)).toBe("");
    expect(expandTemplate("A $10th", "0 1 2 3 4 5 6 7 8 9 ten")).toBe("A tenth");
  });

  /// A position nothing was typed at is left as written, and does not count
  /// as taking the arguments — so they still arrive, at the end.
  test("leaves a position with no argument as written", () => {
    expect(expandTemplate("Fix $0 on $ARGUMENTS[1] or $2", "login")).toBe("Fix login on $ARGUMENTS[1] or $2");
    expect(expandTemplate("Then $3", "a b")).toBe("Then $3\n\nARGUMENTS: a b");
  });

  test("puts a named argument at its position, empty when nothing was typed there", () => {
    const names = ["issue", "branch"];
    expect(expandTemplate("Fix $issue on $branch ($issues)", "123 main", names)).toBe("Fix 123 on main ($issues)");
    expect(expandTemplate("Fix $issue on $branch.", "123", names)).toBe("Fix 123 on .");
    expect(expandTemplate("Only $branch", "", names)).toBe("Only ");
    expect(expandTemplate("No $issue here", "1", [])).toBe("No $issue here\n\nARGUMENTS: 1");
    // A name is whole: `$file` is not the start of `$filename`.
    expect(expandTemplate("$file and $filename", "a b", ["file", "filename"])).toBe("a and b");
  });

  /// One backslash makes a placeholder text; two are text themselves.
  test("leaves an escaped placeholder as text", () => {
    expect(expandTemplate("Costs \\$1.00, not $0", "x")).toBe("Costs $1.00, not x");
    expect(expandTemplate("\\$ARGUMENTS[0] and \\$ARGUMENTS", "x")).toBe("$ARGUMENTS[0] and $ARGUMENTS\n\nARGUMENTS: x");
    expect(expandTemplate("\\\\$0", "x")).toBe("\\\\x");
    expect(expandTemplate("\\$5 stays", "")).toBe("$5 stays");
  });

  /// One pass: what was typed is never read as a template itself.
  test("puts in an argument that looks like a placeholder as text", () => {
    expect(expandTemplate("Summarize $0", `"$ARGUMENTS from $1"`)).toBe("Summarize $ARGUMENTS from $1");
    expect(expandTemplate("Price: $ARGUMENTS", "$& $1")).toBe("Price: $& $1");
  });

  const file = (name: string, template = "Do $ARGUMENTS"): CommandFile => ({
    name,
    description: `about ${name}`,
    argumentHint: name === "review" ? "<file>" : null,
    arguments: name === "fix" ? ["issue"] : [],
    template,
    source: "project",
  });

  /// The transcript shows the command typed; the model gets the prompt.
  test("sends its prompt as the command typed, and never takes a built-in's name", () => {
    const sent: [string, string][] = [];
    const listed = fileCommands([file("review"), file("compact")], commands, (text, prompt) => sent.push([text, prompt]));

    expect(listed.map((c) => [c.name, c.hint, c.argumentHint])).toEqual([["review", "about review", "<file>"]]);
    listed[0].run("src/a.ts");
    listed[0].run("");
    expect(sent).toEqual([
      ["/review src/a.ts", "Do src/a.ts"],
      ["/review", "Do "],
    ]);
  });

  test("names its arguments as its file does", () => {
    const sent: string[] = [];
    fileCommands([file("fix", "Fix issue $issue")], commands, (_, prompt) => sent.push(prompt))[0].run("42");
    expect(sent).toEqual(["Fix issue 42"]);
  });
});

// Read as Vite's `?raw` does, through Bun's own loader.
const initPrompt = (await import("../prompts/init.md", { with: { type: "text" } })).default as string;

describe("the /init prompt", () => {
  /// It is sent as it is: frontmatter would reach the model as text, and a
  /// lost placeholder would put the user's focus after the summary step.
  test("is a prompt with one place for what was typed", () => {
    expect(initPrompt.startsWith("---")).toBe(false);
    expect(initPrompt.split("$ARGUMENTS")).toHaveLength(2);
    expect(expandTemplate(initPrompt, "the IPC layer")).toContain("(may be empty): the IPC layer\n");
  });
});

describe("a command's prompt and the reply language", () => {
  // The prompt is the app's words; what the user typed — the one thing in
  // their language — is not what the model gets, so the prompt says it.
  test("ends by naming the chosen language, and auto adds nothing", async () => {
    const { withLanguageReminder } = await import("../lib/chat");
    expect(withLanguageReminder("Write AGENTS.md.", "russian")).toBe("Write AGENTS.md.\n\n[Reply in Russian.]");
    expect(withLanguageReminder("Write AGENTS.md.", "english")).toBe("Write AGENTS.md.\n\n[Reply in English.]");
    expect(withLanguageReminder("Write AGENTS.md.", "auto")).toBe("Write AGENTS.md.");
  });
});

describe("an MCP server's prompts", () => {
  const prompt = (name: string, args: { name: string; required: boolean }[] = [], extra = {}) => ({
    server: "GitHub",
    name,
    title: null,
    description: "",
    arguments: args.map((a) => ({ ...a, description: "" })),
    ...extra,
  });

  test("are commands named for their server, with their arguments shown", () => {
    const commands = mcpPromptCommands(
      [prompt("Review", [{ name: "pr", required: true }, { name: "focus", required: false }], { title: "Review a PR" }), prompt("plain")],
      [],
      async () => "",
      () => {},
      () => {},
    );
    expect(commands.map((c) => [c.name, c.hint, c.argumentHint])).toEqual([
      ["github:review", "Review a PR — from GitHub", "<pr> [focus]"],
      ["github:plain", "plain — from GitHub", undefined],
    ]);
  });

  test("leave a name already taken to its owner", () => {
    const taken = [{ name: "github:review", hint: "mine", run: () => {} }];
    expect(mcpPromptCommands([prompt("review")], taken, async () => "", () => {}, () => {})).toEqual([]);
  });

  test("send what the server wrote, shown as the command typed; a failure is said, not sent", async () => {
    const sent: [string, string][] = [];
    const failed: string[] = [];
    const asked: string[][] = [];
    const [ok] = mcpPromptCommands(
      [prompt("Review")],
      [],
      async (server, name, args) => {
        asked.push([server, name, args]);
        return "Review PR 42 carefully.";
      },
      (text, body) => sent.push([text, body]),
      (why) => failed.push(why),
    );
    ok.run("42");
    await new Promise((settled) => setTimeout(settled, 0));
    expect(asked).toEqual([["GitHub", "Review", "42"]]);
    expect(sent).toEqual([["/github:review 42", "Review PR 42 carefully."]]);

    const [broken] = mcpPromptCommands([prompt("Review")], [], async () => Promise.reject("not running"), (t, b) => sent.push([t, b]), (why) => failed.push(why));
    broken.run("");
    await new Promise((settled) => setTimeout(settled, 0));
    expect(sent).toHaveLength(1);
    expect(failed[0]).toContain("not running");
  });
});
