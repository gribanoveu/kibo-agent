import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";

// Helper agents: `explore` runs, for the Agents tab.

/** Mirrors `domain::agents::AgentState`. */
export type AgentState = { state: "running" } | { state: "done" } | { state: "failed"; reason: string } | { state: "stopped" };
/** Mirrors `domain::agents::AgentTokens`: what the provider reported, summed over the run. */
export type AgentTokens = { prompt: number; cached: number; completion: number };
/** Mirrors `domain::agents::AgentInfo`: one `explore` run, as the Agents tab shows it. */
export type AgentInfo = {
  id: number;
  task: string;
  state: AgentState;
  steps: string[];
  tokens: AgentTokens;
  answer: string | null;
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

/**
 * What a helper is asked, in one line: the brief's `Goal:` or `Question:`
 * line when it has one — a model often opens with context such as
 * `Repository: /path` — else its first line.
 */
export function briefTitle(task: string): string {
  const lines = task.split("\n").map((line) => line.trim()).filter(Boolean);
  const asked = lines.map((line) => /^(?:goal|question|task|цель|вопрос|задача)\s*:\s*(.+)$/i.exec(line)?.[1]).find(Boolean);
  return asked ?? lines[0] ?? "";
}

/** Newest first. */
export async function agentsList(): Promise<AgentInfo[]> {
  if (!inTauri()) return [];
  return invoke<AgentInfo[]>("agents_list");
}

/** The helper ends at its next check; the turn that started it is told and carries on. */
export async function stopAgent(id: number): Promise<AgentInfo[]> {
  requireBackend();
  return invoke<AgentInfo[]>("agent_stop", { id });
}

/** What a run cost: `38k tokens · 90% cached`; nothing while nothing is reported. */
export function tokensSpent(tokens: AgentTokens): string {
  const total = tokens.prompt + tokens.completion;
  if (!total) return "";
  const shown = total >= 1000 ? `${Math.round(total / 1000)}k` : `${total}`;
  const cached = tokens.prompt ? Math.round((tokens.cached / tokens.prompt) * 100) : 0;
  return cached ? `${shown} tokens · ${cached}% cached` : `${shown} tokens`;
}

/** How a run stands, in a word or two. */
export function agentStatus(state: AgentState): string {
  return state.state === "failed" ? "failed" : state.state;
}
