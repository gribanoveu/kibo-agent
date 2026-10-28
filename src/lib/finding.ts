import type { Block } from "./chatTurnReducer";

// A review's finding is not state of its own: it is a `reportFinding` call
// the backend kept — its arguments say what, its result says where.

export type Severity = "critical" | "high" | "medium" | "low";

export type Finding = {
  path: string;
  startLine: number;
  endLine: number;
  severity: Severity;
  title: string;
  body: string;
  suggestion?: string;
};

type Tool = Extract<Block, { kind: "tool" }>;

/** The finding a settled `reportFinding` call placed; `null` for any other block, or one refused. */
export function findingOf(block: Block): Finding | null {
  if (block.kind !== "tool" || block.name !== "reportFinding" || block.status !== "done") return null;
  const result = block.result as { result?: string; path?: string; startLine?: number; endLine?: number } | undefined;
  if (result?.result !== "findingNoted" || !result.path || !result.startLine) return null;
  let args: Partial<Record<string, unknown>>;
  try {
    args = JSON.parse((block as Tool).arguments);
  } catch {
    return null;
  }
  const text = (key: string) => (typeof args[key] === "string" ? (args[key] as string).trim() : "");
  const severity = text("severity");
  return {
    path: result.path,
    startLine: result.startLine,
    endLine: result.endLine ?? result.startLine,
    severity: (["critical", "high", "medium", "low"].includes(severity) ? severity : "medium") as Severity,
    title: text("title"),
    body: text("body"),
    suggestion: text("suggestion") || undefined,
  };
}

/** A review run as a subagent: what it kept, what it said, and whose findings they are. */
export type Review = {
  findings: Finding[];
  summary: string;
  /** Its record in the Agents tab. */
  agent: number;
  /** The findings went on to the agent to go through, not to the user. */
  handed: boolean;
};

/** The report a settled `/review` card holds; `null` for any other block, or a review that failed. */
export function reviewOf(block: Block): Review | null {
  if (block.kind !== "tool" || block.name !== "review") return null;
  const result = block.result as
    | { result?: string; findings?: (Finding & { suggestion?: string | null })[]; text?: string; agent?: number; handed?: boolean }
    | undefined;
  if (result?.result !== "reviewed") return null;
  return {
    findings: (result.findings ?? []).map((f) => ({ ...f, suggestion: f.suggestion || undefined })),
    summary: (result.text ?? "").trim(),
    agent: result.agent ?? 0,
    handed: result.handed === true,
  };
}
