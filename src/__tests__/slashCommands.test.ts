import { describe, expect, test } from "bun:test";
import {
  commandFor,
  expandTemplate,
  fileCommands,
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

  /// Typed and dropped would be worse than typed and put at the end.
  test("without the placeholder, keeps what was typed after the prompt", () => {
    expect(expandTemplate("Fix the build", "only the linux job")).toBe("Fix the build\n\nonly the linux job");
    expect(expandTemplate("Fix the build", "")).toBe("Fix the build");
  });

  const file = (name: string, template = "Do $ARGUMENTS"): CommandFile => ({
    name,
    description: `about ${name}`,
    argumentHint: name === "review" ? "<file>" : null,
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
