import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

// The tool-call log window.

export type CallStatus = "ok" | "error" | "denied";

/** One settled call, as stored: arguments and result have their file content replaced by `<redacted>`. */
export type ToolLogRow = {
  id: number;
  tsMs: number;
  repoRoot: string;
  round: number;
  providerId: string;
  model: string;
  tool: string;
  /** `null` when the model's arguments did not parse. */
  args: { tool?: string; args?: Record<string, unknown> } | null;
  status: CallStatus;
  error: string | null;
  result: unknown;
  durationMs: number;
};

export type ToolLogFilter = {
  tool?: string;
  status?: CallStatus;
  search?: string;
  limit?: number;
  offset?: number;
};

export type ToolLogPage = { rows: ToolLogRow[]; total: number };

export async function toolLogQuery(filter: ToolLogFilter): Promise<ToolLogPage> {
  if (!inTauri()) return { rows: [], total: 0 };
  return invoke<ToolLogPage>("tool_log_query", { filter });
}

export async function toolLogClear(): Promise<number> {
  requireBackend();
  return invoke<number>("tool_log_clear");
}

export async function toolLogEnabled(): Promise<boolean> {
  if (!inTauri()) return true;
  return invoke<boolean>("tool_log_enabled_get");
}

export async function setToolLogEnabled(enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("tool_log_enabled_set", { enabled });
}
