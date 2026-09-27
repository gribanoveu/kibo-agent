import { describe, expect, test } from "bun:test";
import { collapseRedraws, describeActive, describeRun, describeTool } from "../lib/describeTool";
import type { Block } from "../lib/chatTurnReducer";

// What a tool call looks like in the transcript. Rendering, so the tests are
// about what reads well — and about not falling over on arguments that are
// still half-written.

const tool = (over: Partial<Extract<Block, { kind: "tool" }>>): Extract<Block, { kind: "tool" }> => ({
  kind: "tool",
  id: "c1",
  round: 1,
  name: "readFile",
  arguments: "{}",
  status: "done",
  output: "",
  ...over,
});

describe("what each call shows", () => {
  test("a background start shows its number, not an exit code", () => {
    const waiting = describeTool(tool({ name: "runCommand", arguments: '{"command":"npm run dev","background":true}', status: "running" }));
    expect(waiting).toMatchObject({ name: "Bash", arg: "npm run dev", meta: "background" });
    const started = describeTool(
      tool({
        name: "runCommand",
        arguments: '{"command":"npm run dev","background":true}',
        result: { result: "processStarted", id: 3, command: "npm run dev", cwd: ".", state: { state: "running" } },
      }),
    );
    expect(started.meta).toBe("background #3");
  });

  test("a command moved to the background at its timeout shows its number and what it wrote till then", () => {
    const moved = describeTool(
      tool({
        name: "runCommand",
        arguments: '{"command":"cargo test"}',
        output: "Compiling kibo\n",
        result: { result: "commandMoved", id: 4, command: "cargo test", cwd: ".", state: { state: "running" }, afterMs: 120000 },
      }),
    );
    expect(moved).toMatchObject({ name: "Bash", arg: "cargo test", detail: "Compiling kibo\n", process: 4 });
    expect(moved.meta).toStartWith("moved to background #4 · ");
  });

  test("reading a process shows what it wrote and how it stands", () => {
    const read = describeTool(
      tool({
        name: "readOutput",
        arguments: '{"id":3}',
        result: { id: 3, command: "npm run dev", cwd: ".", state: { state: "exited", code: 1 }, output: "EADDRINUSE", missed: true, truncated: false },
      }),
    );
    expect(read).toMatchObject({ name: "Output", arg: "#3 npm run dev", meta: "exit 1 · some output lost", detail: "EADDRINUSE" });
    const stopped = describeTool(
      tool({ name: "stopProcess", arguments: '{"id":3}', result: { id: 3, command: "npm run dev", cwd: ".", state: { state: "stopped" } } }),
    );
    expect(stopped).toMatchObject({ name: "Stop", arg: "#3 npm run dev", meta: "stopped" });
    const killed = describeTool(
      tool({ name: "readOutput", arguments: '{"id":4}', result: { id: 4, command: "x", cwd: ".", state: { state: "exited", code: null }, output: "" } }),
    );
    expect(killed.meta).toBe("killed by a signal");
  });

  test("a helper's research shows its question, its steps while it runs, then its answer", () => {
    const args = '{"task":"Where is the token refreshed?\\nReport paths and lines."}';
    const running = describeTool(tool({ name: "explore", arguments: args, status: "running", output: "grep refresh\nreadFile src/auth.rs\n" }));
    expect(running).toMatchObject({ name: "Explore", arg: "Where is the token refreshed?", meta: "2 steps", detail: "grep refresh\nreadFile src/auth.rs\n" });
    const done = describeTool(
      tool({
        name: "explore",
        arguments: args,
        output: "grep refresh\n",
        result: { text: "src/auth.rs:42", agent: 3, tokens: { prompt: 12000, cached: 9000, completion: 400 } },
      }),
    );
    expect(done).toMatchObject({ meta: "1 step · 12k tokens · 75% cached · agent #3", detail: "src/auth.rs:42" });
    const failed = describeTool(tool({ name: "explore", arguments: args, error: "explore: stopped before it answered" }));
    expect(failed).toMatchObject({ arg: "Where is the token refreshed?", meta: "failed" });
  });

  test("the user's terminal: which one and what it showed; a typed line and where it went", () => {
    const read = describeTool(
      tool({
        name: "readTerminal",
        arguments: "{}",
        result: { id: 2, shell: "zsh", state: { state: "running" }, alternate: false, output: "$ npm test\n1 failed" },
      }),
    );
    expect(read).toMatchObject({ name: "Screen", arg: "#2 zsh", meta: "2 lines", detail: "$ npm test\n1 failed" });
    const blank = describeTool(
      tool({ name: "readTerminal", arguments: "{}", result: { id: 2, shell: "zsh", state: { state: "running" }, output: "" } }),
    );
    expect(blank.meta).toBeUndefined();
    const typed = describeTool(
      tool({
        name: "runInTerminal",
        arguments: '{"command":"npm run dev"}',
        result: { terminal: { id: 2, shell: "zsh", state: { state: "running" } }, command: "npm run dev" },
      }),
    );
    expect(typed).toMatchObject({ name: "Terminal", arg: "npm run dev", meta: "terminal #2", detail: "" });
  });

  test("a read names the file and the lines it actually returned", () => {
    const shown = describeTool(
      tool({
        name: "readFile",
        arguments: '{"path":"TaxService.java","startLine":119}',
        result: { content: "line\n", startLine: 119, endLine: 124, totalLines: 400 },
      }),
    );

    expect(shown).toMatchObject({ name: "Read", arg: "TaxService.java", meta: "lines 119-124" });
    expect(shown.detail).toBe("line\n");
  });

  test("an outline lists what the file declares, not its text", () => {
    const shown = describeTool(
      tool({
        name: "readFile",
        arguments: '{"path":"lib.rs","outline":true}',
        result: {
          path: "lib.rs",
          entries: [
            { name: "Store", startLine: 5, endLine: 9 },
            { name: "Store.open", startLine: 6, endLine: 6 },
          ],
          totalLines: 9,
        },
      }),
    );

    expect(shown).toMatchObject({ name: "Read", arg: "lib.rs", meta: "outline · 2 entries · 9 lines" });
    expect(shown.detail).toBe("5-9  Store\n6-6  Store.open");
  });

  test("a search counts hits and the files they are in", () => {
    const shown = describeTool(
      tool({
        name: "grep",
        arguments: '{"pattern":"getIncome("}',
        result: {
          matches: [
            { path: "a.java", line: 1, text: "getIncome()" },
            { path: "a.java", line: 9, text: "getIncome()", before: ["// why", "// because"], after: ["}"] },
            { path: "b.java", line: 4, text: "getIncome()" },
          ],
          truncated: false,
        },
      }),
    );

    expect(shown.meta).toBe("3 matches · 2 files");
    const cut = (totalIsFloor: boolean) =>
      describeTool(
        tool({
          name: "grep",
          arguments: '{"pattern":"x"}',
          result: { matches: [{ path: "a.java", line: 1, text: "x" }], truncated: true, total: 347, totalFiles: 58, totalIsFloor },
        }),
      ).meta;
    expect(cut(false)).toBe("1 of 347 matches · 58 files");
    expect(cut(true)).toBe("1 of 347+ matches · 58 files");
    // Grouped by file, as grep prints it, with the lines around a hit.
    expect(shown.detail).toBe("a.java\n1: getIncome()\n--\n7- // why\n8- // because\n9: getIncome()\n10- }\n\nb.java\n4: getIncome()");
  });

  test("overlapping context is shown once, and a line that is a hit shows as one", () => {
    const shown = describeTool(
      tool({
        name: "grep",
        arguments: '{"pattern":"retry"}',
        result: {
          matches: [
            { path: "t.java", line: 118, text: "retry()", before: ["a"], after: ["c", "retry()"] },
            { path: "t.java", line: 120, text: "retry()", before: ["c"], after: ["d"] },
            { path: "t.java", line: 130, text: "retry()" },
          ],
          truncated: false,
        },
      }),
    );
    expect(shown.detail).toBe("t.java\n117- a\n118: retry()\n119- c\n120: retry()\n121- d\n--\n130: retry()");
  });

  /// A capped search that reads as exhaustive is the one thing grep must not
  /// do — the backend says so, and the row has to pass it on.
  test("a capped search says there are more", () => {
    const shown = describeTool(
      tool({
        name: "grep",
        result: { matches: [{ path: "a", line: 1, text: "x" }], truncated: true },
      }),
    );
    expect(shown.meta).toContain("+");
  });

  test("a search shows where each match is and passes the hint on", () => {
    const shown = describeTool(
      tool({
        name: "semanticSearch",
        arguments: '{"query":"where the index syncs"}',
        result: {
          matches: [
            { path: "src/index_sync.rs", startLine: 112, endLine: 130, name: "RepoIndexer.sync", source: "symbol" },
            { path: "docs/plan.md", startLine: 4, endLine: 9, name: null, source: "lexical" },
          ],
          meta: { tiersUsed: ["symbol", "lexical"], weak: true, hint: "Name a function." },
        },
      }),
    );

    expect(shown).toMatchObject({ name: "Search", arg: "where the index syncs", meta: "2 matches" });
    expect(shown.detail.split("\n")).toEqual([
      "src/index_sync.rs:112-130  RepoIndexer.sync",
      "docs/plan.md:4-9",
      "",
      "Name a function.",
    ]);

    const reworded = describeTool(
      tool({
        name: "semanticSearch",
        arguments: '{"query":"где синхронизируется индекс","queries":["where the index syncs"]}',
        result: { matches: [{ path: "a.rs", startLine: 1, endLine: 2, name: null }], meta: {} },
      }),
    );
    expect(reworded.detail.split("\n")).toEqual(["also: where the index syncs", "", "a.rs:1-2"]);
  });

  test("a loaded skill shows its instructions and the files beside them", () => {
    const shown = describeTool(
      tool({
        name: "skill",
        arguments: '{"name":"release"}',
        result: { name: "release", instructions: "Bump the version.\n", files: ["checklist.md"] },
      }),
    );

    expect(shown).toMatchObject({ name: "Skill", arg: "release", meta: "1 file" });
    expect(shown.detail).toBe("Bump the version.\n\n· checklist.md");
  });

  test("a skill's file is shown by the skill and its path", () => {
    const shown = describeTool(
      tool({
        name: "skill",
        arguments: '{"name":"release","path":"checklist.md"}',
        result: { name: "release", path: "checklist.md", content: "1. tag" },
      }),
    );

    expect(shown).toMatchObject({ name: "Skill", arg: "release/checklist.md", detail: "1. tag" });
    expect(shown.meta).toBeUndefined();
  });

  test("a failed search still shows what was asked", () => {
    const shown = describeTool(
      tool({ name: "semanticSearch", arguments: '{"query":"x"}', error: "search is unavailable" }),
    );
    expect(shown).toMatchObject({ name: "Search", arg: "x", meta: "failed" });
  });

  test("a write shows the size of the change and the diff itself", () => {
    const shown = describeTool(
      tool({
        name: "editFile",
        arguments: '{"path":"Mapper.java"}',
        result: { path: "Mapper.java", diff: { linesAdded: 4, linesRemoved: 1, unifiedDiff: "-a\n+b\n" } },
      }),
    );

    expect(shown).toMatchObject({ name: "Edit", arg: "Mapper.java", meta: "+4 -1", diff: true });
    expect(shown.detail).toContain("+b");
  });

  test("a git diff is drawn as a diff, like a write", () => {
    const shown = describeTool(
      tool({
        name: "gitDiff",
        arguments: '{"path":"AGENTS.md"}',
        result: { path: "AGENTS.md", label: "index → working tree", isBinary: false, diff: { linesAdded: 3, linesRemoved: 3, unifiedDiff: "-a\n+b\n" } },
      }),
    );
    expect(shown).toMatchObject({ name: "Diff", arg: "AGENTS.md", meta: "+3 -3", detail: "-a\n+b\n", diff: true });

    const binary = describeTool(
      tool({ name: "gitDiff", arguments: '{"path":"logo.png"}', result: { path: "logo.png", label: "x", isBinary: true, diff: {} } }),
    );
    expect(binary.meta).toBe("binary");
  });

  test("git status names the branch, counts the changes and groups them", () => {
    const shown = describeTool(
      tool({
        name: "gitStatus",
        arguments: "{}",
        result: {
          branch: "main",
          staged: [{ path: "b.rs", status: "A" }],
          unstaged: [{ path: "a.rs", status: "M" }],
          conflicted: [],
          truncated: false,
        },
      }),
    );
    expect(shown).toMatchObject({ name: "Status", arg: "main", meta: "2 changed", detail: "Staged:\n  A b.rs\nNot staged:\n  M a.rs" });

    const clean = describeTool(
      tool({ name: "gitStatus", arguments: "{}", result: { branch: "main", staged: [], unstaged: [], conflicted: [], truncated: false } }),
    );
    expect(clean.meta).toBe("clean");

    const ahead = describeTool(
      tool({
        name: "gitStatus",
        arguments: "{}",
        result: { branch: "main", upstream: { name: "origin/main", ahead: 2, behind: 0 }, staged: [], unstaged: [], conflicted: [], truncated: false },
      }),
    );
    expect(ahead.meta).toBe("clean · ↑2 ↓0");
    const even = describeTool(
      tool({
        name: "gitStatus",
        arguments: "{}",
        result: { branch: "main", upstream: { name: "origin/main", ahead: 0, behind: 0 }, staged: [], unstaged: [], conflicted: [], truncated: false },
      }),
    );
    expect(even.meta).toBe("clean");
  });

  test("a log is one line per commit, with what it was narrowed by", () => {
    const shown = describeTool(
      tool({
        name: "gitLog",
        arguments: '{"path":"src","query":"fix"}',
        result: {
          path: "src",
          commits: [{ commit: "abc12345", date: "2026-09-01", author: "Ann", summary: "fix it", files: 2 }],
          truncated: true,
        },
      }),
    );
    expect(shown).toMatchObject({ name: "Log", arg: 'src "fix"', meta: "1+ commits", detail: "abc12345  2026-09-01  Ann  fix it" });
    const whole = describeTool(tool({ name: "gitLog", arguments: "{}", result: { path: ".", commits: [], truncated: false } }));
    expect(whole).toMatchObject({ arg: "", meta: "0 commits", detail: "" });
  });

  test("blame is one line per run of lines, not JSON", () => {
    const shown = describeTool(
      tool({
        name: "gitBlame",
        arguments: '{"path":"a.rs"}',
        result: {
          path: "a.rs",
          hunks: [{ startLine: 3, lineCount: 2, commit: "abc1234", author: "Ann", date: "2026-09-01", summary: "fix" }],
          truncated: true,
        },
      }),
    );
    expect(shown).toMatchObject({ arg: "a.rs", meta: "1+ hunks", detail: "3-4  abc1234  2026-09-01  Ann  fix" });
  });

  test("a folder made or removed shows its path and nothing to open", () => {
    const shown = describeTool(tool({ name: "createDirectory", arguments: '{"path":"src/new"}', result: { path: "src/new" } }));
    expect(shown).toMatchObject({ name: "Mkdir", arg: "src/new", detail: "" });
  });

  test("a directory diff counts its files and draws each under a header", () => {
    const shown = describeTool(
      tool({
        name: "gitDiff",
        arguments: '{"path":"src"}',
        result: {
          path: "src",
          label: "index → working tree",
          truncated: false,
          files: [
            { path: "src/a.rs", isBinary: false, diff: { linesAdded: 1, linesRemoved: 1, unifiedDiff: "@@ -1 +1 @@\n-a\n+b\n" } },
            { path: "src/logo.png", isBinary: true, diff: { linesAdded: 0, linesRemoved: 0, unifiedDiff: "" } },
            { path: "src/c.rs", isBinary: false, diff: { linesAdded: 2, linesRemoved: 0, unifiedDiff: "@@ -0,0 +1,2 @@\n+x\n+y\n" } },
          ],
        },
      }),
    );
    expect(shown).toMatchObject({ name: "Diff", arg: "src", meta: "3 files +3 -1", diff: true });
    expect(shown.detail).toBe("--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-a\n+b\n--- a/src/c.rs\n+++ b/src/c.rs\n@@ -0,0 +1,2 @@\n+x\n+y\n");
  });

  test("a command says how long it took next to how it ended", () => {
    const ran = (durationMs: number) =>
      describeTool(tool({ name: "runCommand", arguments: '{"command":"make"}', result: { stdout: "", stderr: "", exitCode: 0, timedOut: false, durationMs } })).meta;
    expect(ran(2345)).toBe("exit 0 · 2.3 s");
    expect(ran(40)).toBe("exit 0 · 40 ms");
    expect(ran(0)).toBe("exit 0");
  });

  test("a command shows its exit code", () => {
    const shown = describeTool(
      tool({
        name: "runCommand",
        arguments: '{"command":"./gradlew test"}',
        result: { stdout: "BUILD SUCCESSFUL\n", stderr: "", exitCode: 0, timedOut: false },
      }),
    );

    expect(shown).toMatchObject({ name: "Bash", arg: "./gradlew test", meta: "exit 0" });
    expect(shown.detail).toContain("BUILD SUCCESSFUL");
  });

  /// A killed command has no exit code at all, and "exit 0" would read as
  /// success.
  test("a command killed by its timeout says that instead", () => {
    const shown = describeTool(
      tool({
        name: "runCommand",
        arguments: '{"command":"sleep 300"}',
        result: { stdout: "", stderr: "", exitCode: null, timedOut: true },
      }),
    );

    expect(shown.meta).toBe("timed out");
  });

  test("a running command shows what has streamed so far", () => {
    const shown = describeTool(
      tool({
        name: "runCommand",
        arguments: '{"command":"cargo test"}',
        status: "running",
        result: undefined,
        output: "Compiling…\n",
      }),
    );

    expect(shown.detail).toBe("Compiling…\n");
    expect(shown.meta).toBeUndefined();
  });

  test("a failed call shows the same message the model was given", () => {
    const shown = describeTool(
      tool({
        name: "writeFile",
        arguments: '{"path":"a.rs"}',
        status: "failed",
        error: "Error: read a.rs before writing to it",
      }),
    );

    expect(shown.meta).toBe("failed");
    expect(shown.detail).toContain("before writing");
  });

  test("a move names both ends", () => {
    const shown = describeTool(
      tool({ name: "move", arguments: '{"path":"old.rs","newPath":"new.rs"}' }),
    );
    expect(shown.arg).toBe("old.rs → new.rs");
  });
});

describe("a connected server's tool", () => {
  test("is named by its server and tool, and shows the text it answered", () => {
    const shown = describeTool(
      tool({ name: "mcp__tracker__find_issues", arguments: '{"query":"crash"}', result: { result: "mcp", text: "2 issues" } }),
    );
    expect(shown).toMatchObject({ name: "tracker · find_issues", arg: '{"query":"crash"}', detail: "2 issues" });
  });

  test("a failure says so under the same name", () => {
    const shown = describeTool(tool({ name: "mcp__tracker__find", error: "the tool reported an error: no access" }));
    expect(shown).toMatchObject({ name: "tracker · find", meta: "failed" });
  });
});

describe("arguments that are not finished yet", () => {
  /// The normal case while the model is still writing the call: the row has to
  /// draw something rather than throw.
  test("half-written JSON draws an empty argument, not an exception", () => {
    const shown = describeTool(tool({ name: "writeFile", arguments: '{"path":"a.r', status: "running" }));
    expect(shown.name).toBe("Write");
    expect(shown.arg).toBe("");
  });

  test("a tool this build has never heard of is shown by its own name", () => {
    const shown = describeTool(tool({ name: "summonDragon", arguments: "{}" }));
    expect(shown.name).toBe("summonDragon");
  });
});

describe("a run of calls in one line", () => {
  test("counts each kind in the order it first happened", () => {
    const run = [
      tool({ name: "readFile" }),
      tool({ name: "grep" }),
      tool({ name: "readFile" }),
      tool({ name: "runCommand" }),
    ];
    expect(describeRun(run)).toBe("Read 2 files, searched a pattern, ran a command");
  });

  test("a tool without a phrase is counted under its own name", () => {
    expect(describeRun([tool({ name: "mcp__jira__search" }), tool({ name: "mcp__jira__search" })])).toBe(
      "Jira · search ×2",
    );
  });
});

describe("a call under way, in words", () => {
  test("says what it is doing to what", () => {
    expect(describeActive(tool({ name: "readFile", arguments: '{"path":"a.rs"}' }))).toBe("Reading a.rs");
    expect(describeActive(tool({ name: "runCommand", arguments: '{"command":"cargo test"}' }))).toMatch(/^Running cargo test/);
  });
});

describe("collapseRedraws", () => {
  test("a carriage return writes over the line, as a terminal would", () => {
    expect(collapseRedraws("\rleft: 60 s\rleft: 59 s\rleft:  9 s")).toBe("left:  9 s");
    expect(collapseRedraws("50% done\r75%")).toBe("75% done");
    expect(collapseRedraws("step 1\n42%\r")).toBe("step 1\n42%");
    expect(collapseRedraws("a\r\nb\r\n")).toBe("a\nb\n");
    expect(collapseRedraws("plain")).toBe("plain");
  });

  test("a running command's live output shows the last redraw", () => {
    const running = tool({ name: "runCommand", arguments: '{"command":"timer"}', status: "running", output: "go\n\rleft: 60\rleft: 59" });
    expect(describeTool(running).detail).toBe("go\nleft: 59");
  });
});

