import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

// Settings: providers, keys, web search, the shell, how replies are written.

/** The wire protocol an endpoint speaks. Absent means OpenAI-compatible. */
export type ProviderKind = "openAiCompatible" | "anthropic";

export type ProviderConfig = {
  id: string;
  kind?: ProviderKind;
  baseUrl: string;
  /** Where the model list is asked for. Unset is `{baseUrl}/models`. */
  modelsUrl?: string | null;
  model?: string | null;
  trustedCertPem?: string | null;
  requestHeaders?: Record<string, string>;
  temperature?: number | null;
  /** Nucleus sampling. Unset sends nothing, leaving the model's default. */
  topP?: number | null;
  maxTokens?: number | null;
  /** The model's context window in tokens. Unset means the app does not know. */
  contextLimit?: number | null;
  reasoningEffort?: string | null;
  /** Whether attached pictures are sent. Unset is off: each goes as a note that it was left out. */
  supportsImages?: boolean;
};

/** A provider as the window sees it: the configuration, plus whether a key is stored. */
export type ProviderView = ProviderConfig & { hasApiKey: boolean };

/** Mirrors `domain::settings::ReplyLanguage`: what the model writes in; `auto` asks nothing. */
export type ReplyLanguage = "auto" | "english" | "russian";

/**
 * A `/` command's prompt with the reply language said at its end, as
 * `domain::prompt::with_language_reminder` does for the app's own notes: the
 * prompt is the app's words, usually English, and what the user typed — the
 * one thing in their language — is not what the model gets.
 */
export function withLanguageReminder(prompt: string, language: ReplyLanguage): string {
  if (language === "auto") return prompt;
  const name = language === "english" ? "English" : "Russian";
  return `${prompt}\n\n[Reply in ${name}.]`;
}

/** Mirrors `domain::settings::TurnLimits`: how far one agent turn may run. */
export type TurnLimits = { rounds: number; budget: number };

export const DEFAULT_TURN_LIMITS: TurnLimits = { rounds: 100, budget: 500 };

/** Mirrors `infra::master_key::KeyStore`: where the key the API keys are sealed under is kept. */
export type KeyStore = "file" | "keychain";

export type LlmSettings = {
  providers: ProviderView[];
  activeProviderId: string | null;
  debugLogging: boolean;
  replyLanguage: ReplyLanguage;
  turnLimits: TurnLimits;
  keyStore: KeyStore;
  /** Why the saved API keys cannot be read, when they cannot: then no provider shows a key, though one was saved. */
  keyError?: string | null;
};

export async function llmSettings(): Promise<LlmSettings> {
  if (!inTauri()) return { providers: [], activeProviderId: null, debugLogging: false, replyLanguage: "auto", turnLimits: DEFAULT_TURN_LIMITS, keyStore: "file" };
  return invoke<LlmSettings>("llm_settings_get");
}

export async function saveProvider(provider: ProviderConfig): Promise<void> {
  requireBackend();
  return invoke<void>("llm_provider_save", { provider });
}

export async function removeProvider(id: string): Promise<void> {
  requireBackend();
  return invoke<void>("llm_provider_remove", { id });
}

/**
 * Sends the key to be sealed. There is no command that reads one back — an
 * empty string deletes the stored one.
 */
export async function saveApiKey(id: string, key: string): Promise<void> {
  requireBackend();
  return invoke<void>("llm_api_key_save", { id, key });
}

/** Where a chat's web search goes: nowhere, the user's SearXNG, or Tavily with the user's key. */
export type WebSearchBackend = "off" | "searxng" | "tavily";

/** Where the web tools are offered, each with its own search: a chat, and an agent in a folder. */
export type WebSearchPlace = "chat" | "agent";

/**
 * The search chosen for `place` in Settings → Web search — or, never chosen: a chat's is
 * Tavily with a key and off without, an agent's is off.
 */
export async function webSearchBackend(place: WebSearchPlace): Promise<WebSearchBackend> {
  requireBackend();
  return invoke<WebSearchBackend>("web_search_backend_get", { place });
}

export async function setWebSearchBackend(place: WebSearchPlace, backend: WebSearchBackend): Promise<void> {
  requireBackend();
  return invoke<void>("web_search_backend_set", { place, backend });
}

/** A shell to pick on Windows, for the agent's commands and the terminal tab. */
export type ShellKind = "cmd" | "windowsPowershell" | "powershell" | "gitBash";

export type ShellChoices = {
  /** Installed here — none off Windows, where there is nothing to pick. */
  options: { kind: ShellKind; program: string }[];
  /** The one in use: the picked one while installed, else Git Bash, else cmd. */
  chosen: ShellKind | null;
};

export async function shellChoices(): Promise<ShellChoices> {
  requireBackend();
  return invoke<ShellChoices>("shell_choices_get");
}

/** From the next command and the next terminal tab on. */
export async function setShell(kind: ShellKind): Promise<void> {
  requireBackend();
  return invoke<void>("shell_set", { kind });
}

/** Where the user's SearXNG answers — `http://localhost:8080/` until one is saved. */
export async function searxngUrl(): Promise<string> {
  requireBackend();
  return invoke<string>("web_search_searxng_url_get");
}

/** Saves the SearXNG address and returns it as kept: with a trailing `/`, without a query. Rejects a non-http(s) one. */
export async function saveSearxngUrl(url: string): Promise<string> {
  requireBackend();
  return invoke<string>("web_search_searxng_url_set", { url });
}

/** Whether a Tavily key is saved for the chat's web search. The key itself never comes back. */
export async function webSearchKeyStatus(): Promise<boolean> {
  requireBackend();
  return invoke<boolean>("web_search_key_status");
}

/** What the saved key has spent this billing cycle; `limit` is null when Tavily sets none. */
export type WebUsage = { plan: string | null; used: number; limit: number | null };

/** "Researcher · 150 of 1,000 credits used this billing cycle, 850 left". */
export function describeUsage({ plan, used, limit }: WebUsage): string {
  const n = (value: number) => value.toLocaleString("en-US");
  const spent =
    limit === null
      ? `${n(used)} credits used this billing cycle`
      : `${n(used)} of ${n(limit)} credits used this billing cycle, ${n(Math.max(limit - used, 0))} left`;
  return plan ? `${plan} · ${spent}` : spent;
}

/** Asks Tavily what the saved key has spent — `null` without a key. Rejects when Tavily refuses the key. */
export async function webSearchUsage(): Promise<WebUsage | null> {
  requireBackend();
  return invoke<WebUsage | null>("web_search_usage");
}

/** Seals the Tavily key; an empty string deletes the stored one. */
export async function saveWebSearchKey(key: string): Promise<void> {
  requireBackend();
  return invoke<void>("web_search_key_save", { key });
}

/** Moves the master key to `store`. The system may ask the user before the keychain answers. */
export async function setKeyStore(store: KeyStore): Promise<void> {
  requireBackend();
  return invoke<void>("llm_key_store_set", { store });
}

export async function setActiveProvider(id: string | null): Promise<void> {
  requireBackend();
  return invoke<void>("llm_active_provider_set", { id });
}

export async function setDebugLogging(enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("llm_debug_logging_set", { enabled });
}

export async function setReplyLanguage(language: ReplyLanguage): Promise<void> {
  requireBackend();
  return invoke<void>("llm_reply_language_set", { language });
}

/** From the next turn on; each limit is at least 1. */
export async function setTurnLimits(limits: TurnLimits): Promise<void> {
  requireBackend();
  return invoke<void>("llm_turn_limits_set", { limits });
}

/** A live call, so it is also what proves the URL and the key are both right. */
export async function listModels(id?: string): Promise<string[]> {
  requireBackend();
  return invoke<string[]>("llm_models_list", { id: id ?? null });
}

/**
 * What `provider` serves, as the form has it now — saved or not. `apiKey`
 * `null` uses the stored key.
 */
export async function probeModels(provider: ProviderConfig, apiKey: string | null): Promise<string[]> {
  requireBackend();
  return invoke<string[]>("llm_models_probe", { provider, apiKey });
}
