import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";
import { compactTokens } from "./usage";

// Helper agents: `explore` runs, for the Agents tab.

/** Mirrors `domain::agents::AgentState`. */
export type AgentState = { state: "running" } | { state: "done" } | { state: "failed"; reason: string } | { state: "stopped" };
/** Mirrors `domain::agents::AgentTokens`: what the provider reported, summed over the run. */
export type AgentTokens = { prompt: number; cached: number; completion: number };
/** Mirrors `domain::agents::AgentProgress`: the stream under way, for the live meter. */
export type AgentProgress = {
  /** When the round under way streamed its first token (ms since the epoch); `null` before it has. */
  streamSince: number | null;
  /** Characters the round under way has streamed: text, reasoning, call arguments. */
  streamChars: number;
  /** The finished rounds' tokens, streaming time and characters the ratio comes from. */
  timed: { tokens: number; ms: number; chars: number };
};
/** Mirrors `domain::agents::AgentInfo`: one `explore` run, as the Agents tab shows it. */
export type AgentInfo = {
  id: number;
  task: string;
  state: AgentState;
  steps: string[];
  tokens: AgentTokens;
  answer: string | null;
  progress: AgentProgress;
};

/**
 * A helper agent started, took a step, spent tokens or ended. Pinned on the
 * Rust side in `commands/agents.rs`.
 */
export const AGENT_EVENT = "agents:changed";

export async function onAgentChanged(handler: (id: number) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ id: number }>(AGENT_EVENT, ({ payload }) => handler(payload.id));
}

/** A review brief's focus line — `domain::review::review_message`, whose tests pin the wording. */
const REVIEW_FOCUS = "The one asking for this review asks to pay particular attention to: ";

/**
 * What a helper is asked, in one line: the brief's `Goal:` or `Question:`
 * line when it has one — a model often opens with context such as
 * `Repository: /path` — a review's `focus`, when the caller gave one, since
 * its first line is the same for every review — else its first line.
 */
export function briefTitle(task: string): string {
  const lines = task.split("\n").map((line) => line.trim()).filter(Boolean);
  // A label is the brief's own line, written at the start of a line — so the
  // match is against the untrimmed line. Embedded material never is: a diff's
  // lines carry `+`/`-` or a context space, a code snippet is indented, and
  // `task: …` in either must not be read as the brief's `Task:`.
  const asked = task.split("\n").map((line) => /^(?:goal|question|task|цель|вопрос|задача)\s*:\s*(.+)$/i.exec(line)?.[1].trim()).find(Boolean);
  // The template's trailing dot is not the focus's own.
  const focus = lines.find((line) => line.startsWith(REVIEW_FOCUS))?.slice(REVIEW_FOCUS.length).replace(/\.$/, "");
  return asked ?? focus ?? lines[0] ?? "";
}

/** The runs chat `chatId` started, newest first. */
export async function agentsList(chatId: string): Promise<AgentInfo[]> {
  if (!inTauri()) return [];
  return invoke<AgentInfo[]>("agents_list", { chatId });
}

/** The helper ends at its next check; the turn that started it is told and carries on. Answers with chat `chatId`'s list. */
export async function stopAgent(id: number, chatId: string): Promise<AgentInfo[]> {
  requireBackend();
  return invoke<AgentInfo[]>("agent_stop", { id, chatId });
}

/** What a run cost: `38k tokens · 90% cached`; nothing while nothing is reported. */
export function tokensSpent(tokens: AgentTokens): string {
  const total = tokens.prompt + tokens.completion;
  if (!total) return "";
  const shown = compactTokens(total);
  const cached = tokens.prompt ? Math.round((tokens.cached / tokens.prompt) * 100) : 0;
  return cached ? `${shown} tokens · ${cached}% cached` : `${shown} tokens`;
}

/** How a run stands, in a word or two. */
export function agentStatus(state: AgentState): string {
  return state.state === "failed" ? "failed" : state.state;
}

/** Characters per token of the run's finished rounds — or the chat's own guess
    before there is one (`liveSpeed` in `chatTurnReducer.ts`). */
const charsPerToken = ({ progress: { timed } }: AgentInfo) => (timed.chars > 0 && timed.tokens > 0 ? timed.chars / timed.tokens : 3.5);

/**
 * The run's output speed while its round streams, the chat's own estimate:
 * characters turned into tokens at the finished rounds' ratio. `null` for the
 * first second, when a few characters say nothing.
 */
export function agentSpeed(a: AgentInfo, now: number): number | null {
  const { streamSince, streamChars } = a.progress;
  if (streamSince === null || now - streamSince < 1000) return null;
  return (streamChars / charsPerToken(a)) * (1000 / (now - streamSince));
}

/** What the run has spent so far: every round reported, the one under way estimated. */
export function tokensLive(a: AgentInfo): number {
  return a.tokens.prompt + a.tokens.completion + Math.round(a.progress.streamChars / charsPerToken(a));
}

/**
 * The Working panel's subagent line: the running ones' summed speed and
 * tokens, or `null` with none running. `speed` is `null` until one has an
 * estimate; `streaming` says the tokens carry an estimate of their own.
 */
export function subagentMeter(
  agents: AgentInfo[],
  now: number,
): { count: number; speed: number | null; tokens: number; streaming: boolean } | null {
  const running = agents.filter((a) => a.state.state === "running");
  if (running.length === 0) return null;
  const speeds = running.map((a) => agentSpeed(a, now)).filter((rate): rate is number => rate !== null);
  return {
    count: running.length,
    speed: speeds.length ? speeds.reduce((sum, rate) => sum + rate, 0) : null,
    tokens: running.reduce((sum, a) => sum + tokensLive(a), 0),
    streaming: running.some((a) => a.progress.streamSince !== null),
  };
}

/** A step as the helper logged it, `readFile src/auth.rs`: the tool, then what it was called on. */
export function splitStep(step: string): { tool: string; on: string } {
  const space = step.indexOf(" ");
  return space < 0 ? { tool: step, on: "" } : { tool: step.slice(0, space), on: step.slice(space + 1) };
}
