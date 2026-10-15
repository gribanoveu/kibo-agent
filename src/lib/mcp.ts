import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

// MCP servers: their configuration, their state and their prompts.

/** Mirrors `domain::mcp::Exposure`: declared to the model, found through `toolSearch`, or not offered. */
export type McpExposure = "direct" | "deferred" | "hidden";

/**
 * Mirrors `domain::mcp::McpToolInfo`: one tool a running server offers. `readOnly` and `destructive`
 * are the server's own word about it — shown, never trusted. `exposure` is the user's entry's.
 */
export type McpToolInfo = {
  name: string;
  description: string;
  title?: string | null;
  readOnly?: boolean;
  destructive?: boolean;
  exposure?: McpExposure;
};

/** Mirrors `domain::mcp::McpServerState`: what the server's process is doing. */
export type McpServerState =
  | { state: "notStarted" }
  | { state: "starting" }
  | { state: "running"; tools: McpToolInfo[]; instructions?: string | null }
  | { state: "exited"; error: string }
  | { state: "failed"; error: string };

/**
 * Mirrors `domain::mcp::McpServerItem`. `command` is the URL for an HTTP server. `error` is why it
 * will not start, when the entry alone says; `warning` is something wrong that does not stop it.
 */
export type McpServerItem = {
  name: string;
  command: string;
  enabled: boolean;
  error: string | null;
  warning: string | null;
  state: McpServerState;
};
/** The file (`text`, for the editor), where it is, and its servers as rows. */
export type McpView = { path: string; text: string; servers: McpServerItem[] };

/** Mirrors `domain::mcp::McpFieldKind`. */
export type McpFieldKind =
  | { type: "text" }
  | { type: "number"; integer: boolean }
  | { type: "boolean" }
  | { type: "choice"; values: string[]; labels: string[] };

/** Mirrors `domain::mcp::McpField`: one field of a server's form. */
export type McpField = {
  name: string;
  label: string;
  description: string | null;
  required: boolean;
  kind: McpFieldKind;
  default: unknown;
};

/** Mirrors `domain::mcp::McpQuestion`: what an MCP server asks the user mid-call. */
export type McpQuestion =
  | { mode: "form"; message: string; fields: McpField[] }
  | { mode: "url"; message: string; url: string };

/** Mirrors `domain::mcp::McpAnswer`. */
export type McpAnswer =
  | { action: "accept"; content?: Record<string, unknown> }
  | { action: "decline" }
  | { action: "cancel" };

/** Answers question `id` of a running turn; refused when its call has already ended. */
export async function answerMcpQuestion(id: string, answer: McpAnswer): Promise<void> {
  requireBackend();
  await invoke("chat_answer_mcp_question", { id, answer });
}

/** Mirrors `commands::mcp::McpPromptItem`: a prompt a running server offers. */
export type McpPromptItem = {
  server: string;
  name: string;
  title: string | null;
  description: string;
  arguments: { name: string; description: string; required: boolean }[];
};

/** The prompts of the servers running for the open folder; none are started to be asked. */
/** The same, once the servers not running yet are started — slow the first time: a server may be downloaded. */
export async function startMcpPrompts(): Promise<McpPromptItem[]> {
  requireBackend();
  return invoke<McpPromptItem[]>("mcp_prompts_start");
}

export async function mcpPrompts(): Promise<McpPromptItem[]> {
  if (!inTauri()) return [];
  return invoke<McpPromptItem[]>("mcp_prompts");
}

/** A server's prompt written with what was typed after its name. */
export async function mcpPromptGet(server: string, name: string, typed: string): Promise<string> {
  requireBackend();
  return invoke<string>("mcp_prompt_get", { server, name, typed });
}

export async function mcpConfig(): Promise<McpView> {
  if (!inTauri()) return { path: "", text: "", servers: [] };
  return invoke<McpView>("mcp_config_get");
}

/** Refused, and the file left alone, when the text is not a valid `mcpServers` config. */
export async function saveMcpConfig(text: string): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_config_save", { text });
}

/** Starts the server if it is not running, so the tab can show its tools without an Agent turn. */
export async function connectMcpServer(name: string): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_server_connect", { name });
}

export async function setMcpServerEnabled(name: string, enabled: boolean): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_server_set_enabled", { name, enabled });
}

/** Offers one tool of a server to the model, or hides it; the server keeps running. */
export async function setMcpToolShown(server: string, tool: string, shown: boolean): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_tool_set_shown", { server, tool, shown });
}
