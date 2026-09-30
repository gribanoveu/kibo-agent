import type { Block } from "./chatTurnReducer";
import { processStatus, tokensSpent, type AgentTokens, type ProcessState } from "./chat";

// One tool call, as a row in the transcript: what it did, to what, and the
// body you get when you expand it.
//
// Rendering, not logic — which is why it lives on this side. The backend sends
// the call's raw arguments and its typed result; turning `{"path":"a.rs",
// "startLine":119}` into `a.rs · lines 119-124` is a question about what reads
// well in a list, and the answer changes without the protocol changing.
//
// Everything here is defensive: arguments arrive a character at a time while
// the model is still writing them, so half of them are not valid JSON yet.

export type ToolDisplay = {
  /** Short label, as the prototype spells it: Read, Grep, Edit, Bash. */
  name: string;
  /** What it acted on — the path, the pattern, the command line. */
  arg: string;
  /** The secondary fact: a line range, a match count, a diff size, an exit code. */
  meta?: string;
  /** The expandable body. Empty means there is nothing to expand. */
  detail: string;
  /** `detail` is a unified diff, to be drawn as one rather than as text. */
  diff?: boolean;
  /** A background process this call started: the row opens it in the Terminal tab. */
  process?: number;
  /** The helper run an `explore` call was: the open row offers it in the Agents tab. */
  agent?: AgentFocus;
};

/** A helper run to show in the Agents tab. The task as well as the number:
 * numbers start again when the app does, and an old chat's #3 is not today's. */
export type AgentFocus = { id: number; task: string };

export const LABELS: Record<string, string> = {
  readFile: "Read",
  grep: "Grep",
  listFiles: "List",
  writeFile: "Write",
  editFile: "Edit",
  createDirectory: "Mkdir",
  deleteFile: "Delete",
  deleteDirectory: "Delete",
  move: "Move",
  todo: "Todo",
  gitStatus: "Status",
  gitDiff: "Diff",
  gitBlame: "Blame",
  gitLog: "Log",
  runCommand: "Bash",
  semanticSearch: "Search",
  skill: "Skill",
  writePlan: "Plan",
  readOutput: "Output",
  stopProcess: "Stop",
  readTerminal: "Screen",
  runInTerminal: "Terminal",
  reportFinding: "Finding",
  explore: "Explore",
  kubeList: "Kube list",
  kubeGet: "Kube get",
  kubeEvents: "Events",
  kubeLogs: "Logs",
  kubeTop: "Top",
  kubeFieldHistory: "Field history",
  kubeDiagnose: "Diagnose",
  kubeScale: "Scale",
  kubeUndo: "Undo",
  kubeSuspend: "Suspend",
  kubeRolloutRestart: "Restart",
  kubeRolloutUndo: "Roll back",
  kubeApply: "Apply",
  kubeDelete: "Delete object",
};

/** `mcp__<server>__<tool>` as `server · tool`; `null` for any other name. */
export function mcpParts(wireName: string): { server: string; tool: string } | null {
  if (!wireName.startsWith("mcp__")) return null;
  const rest = wireName.slice("mcp__".length);
  const cut = rest.indexOf("__");
  return cut < 0 ? { server: rest, tool: "" } : { server: rest.slice(0, cut), tool: rest.slice(cut + 2) };
}

/** A tool this build does not know is shown by its wire name rather than hidden. */
export const toolLabel = (wireName: string) => {
  const mcp = mcpParts(wireName);
  if (mcp) return `${mcp.server} · ${mcp.tool}`;
  return LABELS[wireName] ?? wireName;
};

type Json = Record<string, unknown>;

const asObject = (value: unknown): Json => (value && typeof value === "object" ? (value as Json) : {});

/** Partial JSON is the normal case while a call is still being written. */
function parseArguments(raw: string): Json {
  try {
    return asObject(JSON.parse(raw));
  } catch {
    return {};
  }
}

const str = (value: unknown) => (typeof value === "string" ? value : undefined);
const num = (value: unknown) => (typeof value === "number" ? value : undefined);

/**
 * Output as a terminal would leave it: a carriage return goes back to the
 * start of the line and what follows writes over it — a countdown shows its
 * last value. The backend does the same to what it settles
 * (`domain::command_exec::collapse_redraws`); this is for the live stream.
 */
export function collapseRedraws(text: string): string {
  if (!text.includes("\r")) return text;
  return text
    .split("\n")
    .map((line) =>
      line.split("\r").reduce((shown, part) => [...part].concat([...shown].slice([...part].length)).join(""), ""),
    )
    .join("\n");
}

export function describeTool(block: Extract<Block, { kind: "tool" }>): ToolDisplay {
  const args = parseArguments(block.arguments);
  const result = asObject(block.result);
  const name = toolLabel(block.name);

  if (block.error) {
    return { name, arg: primaryArgument(block.name, args), meta: "failed", detail: block.error };
  }

  // A connected server's tool. Its arguments are shown whole: nothing here
  // knows which of a foreign tool's fields matters, and the approval card
  // shows this line — agreeing to a call must not mean agreeing blind.
  if (mcpParts(block.name)) {
    return { name, arg: Object.keys(args).length ? JSON.stringify(args) : "", detail: str(result.text) ?? "" };
  }

  // The Kubernetes role's reads: what they read, and the backend's own text
  // for it — a table, YAML, a log — shaped there for the model.
  if (block.name.startsWith("kube") && block.name in LABELS) {
    const replicas = num(args.replicas);
    // What the change sets, where its arguments say: replicas, a revision, on or off.
    const revision = num(args.toRevision);
    const to =
      replicas !== undefined
        ? `→ ${replicas}`
        : revision !== undefined
          ? `→ revision ${revision}`
          : typeof args.suspend === "boolean"
            ? `→ ${args.suspend ? "suspended" : "active"}`
            : null;
    // A manifest is named by the kinds in it: `Deployment, Service`.
    const kinds = [...(str(args.manifest) ?? "").matchAll(/^kind:\s*(\S+)/gm)].map((found) => found[1]).join(", ");
    const target = [str(args.kind) ?? kinds, str(args.name) ?? str(args.pod) ?? str(args.labelSelector) ?? str(args.changeId), to]
      .filter(Boolean)
      .join(" ");
    const namespace = str(args.namespace);
    return {
      name,
      arg: namespace ? `${target} -n ${namespace}` : target,
      meta: str(result.summary),
      detail: str(result.text) ?? "",
    };
  }

  switch (block.name) {
    case "readFile": {
      if (Array.isArray(result.entries)) {
        const entries = result.entries as Json[];
        return {
          name,
          arg: str(args.path) ?? "",
          meta: `outline · ${entries.length} entries · ${num(result.totalLines) ?? 0} lines`,
          detail: entries.map((e) => `${num(e.startLine)}-${num(e.endLine)}  ${str(e.name)}`).join("\n"),
        };
      }
      const from = num(result.startLine) ?? num(args.startLine);
      const to = num(result.endLine) ?? num(args.endLine);
      return {
        name,
        arg: str(args.path) ?? "",
        meta: from && to ? `lines ${from}-${to}` : undefined,
        detail: str(result.content) ?? "",
      };
    }

    case "grep": {
      const matches = Array.isArray(result.matches) ? (result.matches as Json[]) : [];
      const files = new Set(matches.map((m) => str(m.path)).filter(Boolean));
      return {
        name,
        arg: str(args.pattern) ?? "",
        // "5 of 347" when the backend counted past the cap; "5+" for results
        // saved before it did.
        meta: matches.length
          ? result.truncated && num(result.total)
            ? `${matches.length} of ${num(result.total)}${result.totalIsFloor ? "+" : ""} matches · ${num(result.totalFiles)} files`
            : `${matches.length}${result.truncated ? "+" : ""} matches · ${files.size} files`
          : undefined,
        detail: grepDetail(matches),
      };
    }

    case "semanticSearch": {
      const matches = Array.isArray(result.matches) ? (result.matches as Json[]) : [];
      const meta = asObject(result.meta);
      const lines = matches.map((m) => {
        const name = str(m.name);
        return `${str(m.path)}:${num(m.startLine)}-${num(m.endLine)}${name ? `  ${name}` : ""}`;
      });
      // The hint is what the model was told to do next; the reader should see it too.
      const hint = str(meta.hint);
      // The other wordings searched with it: without them the matches can
      // look unrelated to the one query shown.
      const also = (Array.isArray(args.queries) ? args.queries : []).filter((q): q is string => typeof q === "string");
      return {
        name,
        arg: str(args.query) ?? "",
        meta: block.result === undefined ? undefined : `${matches.length} matches`,
        detail: [...also.map((q) => `also: ${q}`), ...(also.length ? [""] : []), ...lines, ...(hint ? ["", hint] : [])].join("\n"),
      };
    }

    case "listFiles": {
      const entries = Array.isArray(result.entries) ? (result.entries as Json[]) : [];
      return {
        name,
        arg: str(args.path) ?? ".",
        meta: entries.length ? `${entries.length}${result.truncated ? "+" : ""} entries` : undefined,
        detail: entries.map((e) => str(e.path) ?? "").join("\n"),
      };
    }

    case "writeFile":
    case "editFile":
    case "deleteFile": {
      const diff = asObject(result.diff);
      const added = num(diff.linesAdded);
      const removed = num(diff.linesRemoved);
      return {
        name,
        arg: str(args.path) ?? "",
        meta: added === undefined ? undefined : `+${added} -${removed ?? 0}`,
        detail: str(diff.unifiedDiff) ?? "",
        diff: true,
      };
    }

    case "gitDiff": {
      // A directory: every changed file, each under its own header.
      if (Array.isArray(result.files)) {
        const files = result.files as Json[];
        const added = files.reduce((n, f) => n + (num(asObject(f.diff).linesAdded) ?? 0), 0);
        const removed = files.reduce((n, f) => n + (num(asObject(f.diff).linesRemoved) ?? 0), 0);
        return {
          name,
          arg: str(args.path) ?? str(result.path) ?? "",
          meta: `${files.length}${result.truncated ? "+" : ""} ${files.length === 1 ? "file" : "files"} +${added} -${removed}`,
          detail: files
            .map((f) => [str(f.path) ?? "", str(asObject(f.diff).unifiedDiff) ?? ""] as const)
            .filter(([, unified]) => unified)
            .map(([path, unified]) => `--- a/${path}\n+++ b/${path}\n${unified}`)
            .join(""),
          diff: true,
        };
      }
      const diff = asObject(result.diff);
      const added = num(diff.linesAdded);
      return {
        name,
        arg: str(args.path) ?? str(result.path) ?? "",
        meta: result.isBinary === true ? "binary" : added === undefined ? str(result.label) : `+${added} -${num(diff.linesRemoved) ?? 0}`,
        detail: str(diff.unifiedDiff) ?? "",
        diff: true,
      };
    }

    case "createDirectory":
    case "deleteDirectory":
      return { name, arg: str(args.path) ?? "", detail: "" };

    case "gitStatus": {
      const groups: [string, unknown][] = [
        ["Conflicted", result.conflicted],
        ["Staged", result.staged],
        ["Not staged", result.unstaged],
      ];
      const lists = groups.map(([title, files]) => [title, Array.isArray(files) ? (files as Json[]) : []] as const);
      const changed = lists.reduce((n, [, files]) => n + files.length, 0);
      const up = (result.upstream ?? {}) as Json;
      // Only when apart: "↑0 ↓0" on every status is noise.
      const apart = Number(up.ahead) || Number(up.behind) ? ` · ↑${up.ahead} ↓${up.behind}` : "";
      return {
        name,
        arg: str(result.branch) ?? "",
        meta:
          block.result === undefined
            ? undefined
            : (changed ? `${changed}${result.truncated ? "+" : ""} changed` : "clean") + apart,
        detail: lists
          .filter(([, files]) => files.length)
          .map(([title, files]) => [`${title}:`, ...files.map((f) => `  ${str(f.status)} ${str(f.path)}`)].join("\n"))
          .join("\n"),
      };
    }

    case "gitLog": {
      const commits = Array.isArray(result.commits) ? (result.commits as Json[]) : [];
      return {
        name,
        arg: [str(args.path), str(args.query) && `"${str(args.query)}"`].filter(Boolean).join(" "),
        meta: block.result === undefined ? undefined : `${commits.length}${result.truncated ? "+" : ""} commits`,
        detail: commits
          .map((c) => `${str(c.commit)}  ${str(c.date)}  ${str(c.author)}  ${str(c.summary)}`)
          .join("\n"),
      };
    }

    case "gitBlame": {
      const hunks = Array.isArray(result.hunks) ? (result.hunks as Json[]) : [];
      return {
        name,
        arg: str(args.path) ?? "",
        meta: hunks.length ? `${hunks.length}${result.truncated ? "+" : ""} hunks` : undefined,
        detail: hunks
          .map((h) => {
            const start = num(h.startLine) ?? 0;
            const last = start + Math.max(0, (num(h.lineCount) ?? 1) - 1);
            return `${start}-${last}  ${str(h.commit)}  ${str(h.date)}  ${str(h.author)}  ${str(h.summary)}`;
          })
          .join("\n"),
      };
    }

    case "move":
      return {
        name,
        arg: `${str(args.path) ?? ""} → ${str(args.newPath) ?? ""}`,
        detail: "",
      };

    case "runCommand": {
      // Started to run on: the answer is a number, not an exit code.
      if (args.background === true) {
        const id = num(result.id);
        return {
          name,
          arg: str(args.command) ?? "",
          meta: id === undefined ? "background" : `background #${id}`,
          detail: "",
          process: id,
        };
      }
      const streamed = block.output;
      // Still running at its timeout, it went on in the background: what
      // streamed in so far is all the call saw.
      if (result.result === "commandMoved") {
        const id = num(result.id);
        return {
          name,
          arg: str(args.command) ?? "",
          meta: [id === undefined ? "background" : `moved to background #${id}`, took(num(result.afterMs))].filter(Boolean).join(" · "),
          detail: collapseRedraws(streamed),
          process: id,
        };
      }
      const settled = `${str(result.stdout) ?? ""}${str(result.stderr) ?? ""}`;
      const code = num(result.exitCode);
      return {
        name,
        arg: str(args.command) ?? "",
        // Two different facts, and a reader needs to tell them apart: a killed
        // command has no exit code at all, and calling that "exit 0" would read
        // as success.
        meta: [result.timedOut ? "timed out" : code === undefined ? undefined : `exit ${code}`, took(num(result.durationMs))]
          .filter(Boolean)
          .join(" · ") || undefined,
        // While it runs there is only what has streamed in; once it settles the
        // captured output is authoritative — and shorter, being truncated in
        // the middle rather than cut off wherever the turn ended.
        detail: settled || collapseRedraws(streamed),
      };
    }

    case "readOutput":
    case "stopProcess": {
      const state = asObject(result.state);
      return {
        name,
        arg: [num(args.id) === undefined ? "" : `#${num(args.id)}`, str(result.command) ?? ""].filter(Boolean).join(" "),
        meta: state.state === undefined ? undefined : [processStatus(state as ProcessState), result.missed ? "some output lost" : ""].filter(Boolean).join(" · "),
        detail: str(result.output) ?? "",
      };
    }

    // The user's own terminal: which one, and what it showed.
    case "readTerminal": {
      const id = num(result.id);
      const output = str(result.output) ?? "";
      return {
        name,
        arg: id === undefined ? "" : `#${id} ${str(result.shell) ?? ""}`.trim(),
        meta: output ? `${output.split("\n").length} lines` : undefined,
        detail: output,
      };
    }

    // Typed, not run to the end: the terminal is where it goes on.
    case "runInTerminal": {
      const id = num(asObject(result.terminal).id);
      return { name, arg: str(args.command) ?? "", meta: id === undefined ? undefined : `terminal #${id}`, detail: "" };
    }

    case "writePlan": {
      // The text is in the arguments; the result only counts its lines.
      const content = str(args.content) ?? "";
      const title = content.split("\n").find((line) => line.trim())?.replace(/^#+\s*/, "") ?? "";
      return {
        name,
        arg: title,
        meta: num(result.lines) === undefined ? undefined : `${num(result.lines)} lines`,
        detail: content,
      };
    }

    // A helper's research: the brief, what it looked at while it ran, and
    // then its answer.
    case "explore": {
      const steps = block.output.split("\n").filter(Boolean).length;
      const spent = result.tokens ? tokensSpent(result.tokens as AgentTokens) : "";
      const agent = num(result.agent);
      return {
        name,
        arg: primaryArgument(block.name, args),
        meta:
          [steps ? `${steps} ${steps === 1 ? "step" : "steps"}` : "", spent, agent === undefined ? "" : `agent #${agent}`]
            .filter(Boolean)
            .join(" · ") || undefined,
        detail: str(result.text) ?? block.output,
        agent: agent === undefined ? undefined : { id: agent, task: str(args.task) ?? "" },
      };
    }

    // A review's finding: the one refused is what shows here, a kept one
    // stands on its own card (`lib/finding.ts`).
    case "reportFinding":
      return { name, arg: str(args.title) ?? "", meta: str(args.path), detail: str(args.body) ?? "" };

    case "skill": {
      // A skill's files are listed after its instructions, as the model got them.
      const files = Array.isArray(result.files) ? (result.files as string[]) : [];
      const path = str(args.path);
      return {
        name,
        arg: [str(args.name) ?? "", path].filter(Boolean).join("/"),
        meta: files.length ? `${files.length} ${files.length === 1 ? "file" : "files"}` : undefined,
        detail: [(str(result.instructions) ?? str(result.content) ?? "").trimEnd(), files.map((f) => `· ${f}`).join("\n")]
          .filter(Boolean)
          .join("\n\n"),
      };
    }

    case "todo": {
      const tasks = Array.isArray(result.tasks) ? (result.tasks as Json[]) : [];
      const done = tasks.filter((t) => str(t.status) === "completed").length;
      return {
        name,
        arg: str(args.op) ?? "",
        meta: tasks.length ? `${done}/${tasks.length} done` : undefined,
        detail: tasks.map((t) => `${statusMark(str(t.status))} ${str(t.title) ?? ""}`).join("\n"),
      };
    }

    default:
      return {
        name,
        arg: primaryArgument(block.name, args),
        detail: block.result === undefined ? "" : JSON.stringify(block.result, null, 2),
      };
  }
}

/**
 * A file's name once, then `line:` for a hit and `line-` for the lines around
 * it, `--` between groups that do not touch. Each line once: overlapping
 * context is shared, and a line that is a hit shows as one.
 */
function grepDetail(matches: Json[]): string {
  const files: [string, Map<number, [boolean, string]>][] = [];
  for (const m of matches) {
    const path = str(m.path) ?? "";
    if (files[files.length - 1]?.[0] !== path) files.push([path, new Map()]);
    const lines = files[files.length - 1][1];
    const line = num(m.line) ?? 0;
    const before = Array.isArray(m.before) ? (m.before as string[]) : [];
    const after = Array.isArray(m.after) ? (m.after as string[]) : [];
    before.forEach((text, k) => {
      const n = line - before.length + k;
      if (!lines.has(n)) lines.set(n, [false, text]);
    });
    lines.set(line, [true, str(m.text) ?? ""]);
    after.forEach((text, k) => {
      if (!lines.has(line + 1 + k)) lines.set(line + 1 + k, [false, text]);
    });
  }
  return files
    .map(([path, lines]) => {
      const out = [path];
      let previous: number | undefined;
      for (const n of [...lines.keys()].sort((a, b) => a - b)) {
        if (previous !== undefined && n > previous + 1) out.push("--");
        const [hit, text] = lines.get(n)!;
        out.push(`${n}${hit ? ":" : "-"} ${text}`);
        previous = n;
      }
      return out.join("\n");
    })
    .join("\n\n");
}

/** `2.3 s`, `40 ms`; nothing when the time is not known. */
function took(ms: number | undefined): string | undefined {
  if (!ms) return undefined;
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
}

function primaryArgument(wireName: string, args: Json): string {
  return (
    str(args.path) ??
    str(args.command) ??
    str(args.pattern) ??
    str(args.query) ??
    // `explore`'s brief: its first line is the question.
    str(args.task)?.split("\n")[0] ??
    (wireName === "gitStatus" ? "" : "")
  );
}

function statusMark(status: string | undefined) {
  if (status === "completed") return "✓";
  if (status === "cancelled") return "✗";
  if (status === "inProgress") return "→";
  return "·";
}

// What a call is doing while it runs, for the folded line: "Reading a.rs",
// "Running cargo test". A label without a verb here keeps its own name.
const ACTIVE_VERBS: Record<string, string> = {
  Read: "Reading",
  Grep: "Searching",
  Search: "Searching the code for",
  List: "Listing",
  Write: "Writing",
  Edit: "Editing",
  Delete: "Deleting",
  Mkdir: "Creating",
  Move: "Moving",
  Bash: "Running",
  Todo: "Updating the checklist",
  Plan: "Writing the plan",
  Finding: "Reporting",
  Status: "Checking git status",
  Diff: "Reading the diff",
  Blame: "Reading the blame of",
  Output: "Reading the output of",
  Stop: "Stopping",
  Explore: "Exploring",
};

export function describeActive(block: Extract<Block, { kind: "tool" }>): string {
  const { name, arg } = describeTool(block);
  return [ACTIVE_VERBS[name] ?? name, arg].filter(Boolean).join(" ");
}

// A run of calls folded into one line, the way a person would say it: "Read 3
// files, searched 2 patterns, ran a command". Each verb takes the count; a
// label without a phrase here is counted under its own name.
const RUN_PHRASES: Record<string, (n: number) => string> = {
  Read: (n) => `read ${n === 1 ? "a file" : `${n} files`}`,
  Grep: (n) => `searched ${n === 1 ? "a pattern" : `${n} patterns`}`,
  Search: (n) => `searched the code${n > 1 ? ` ${n} times` : ""}`,
  List: (n) => `listed ${n === 1 ? "a folder" : `${n} folders`}`,
  Write: (n) => `wrote ${n === 1 ? "a file" : `${n} files`}`,
  Edit: (n) => `edited ${n === 1 ? "a file" : `${n} files`}`,
  Delete: (n) => `deleted ${n === 1 ? "an item" : `${n} items`}`,
  Mkdir: (n) => `created ${n === 1 ? "a folder" : `${n} folders`}`,
  Move: (n) => `moved ${n === 1 ? "an item" : `${n} items`}`,
  Bash: (n) => `ran ${n === 1 ? "a command" : `${n} commands`}`,
  Todo: () => "updated the checklist",
  Plan: () => "wrote the plan",
  Finding: (n) => `reported ${n === 1 ? "a finding" : `${n} findings`}`,
  Explore: (n) => `explored ${n === 1 ? "a question" : `${n} questions`}`,
  "Kube list": (n) => `listed ${n === 1 ? "a kind" : `${n} kinds`}`,
  "Kube get": (n) => `read ${n === 1 ? "an object" : `${n} objects`}`,
  Events: () => "read events",
  Logs: (n) => `read ${n === 1 ? "a log" : `${n} logs`}`,
  Scale: (n) => `scaled ${n === 1 ? "a workload" : `${n} workloads`}`,
  Diagnose: (n) => `diagnosed ${n === 1 ? "a workload" : `${n} workloads`}`,
};

export function describeRun(tools: Extract<Block, { kind: "tool" }>[]): string {
  const counts = new Map<string, number>();
  for (const tool of tools) {
    const name = toolLabel(tool.name);
    counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  const parts = [...counts].map(([name, n]) => RUN_PHRASES[name]?.(n) ?? (n === 1 ? name : `${name} ×${n}`));
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}
