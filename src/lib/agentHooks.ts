import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

// The user's hooks: their configuration, for the Hooks tab.

/** Mirrors `domain::hooks::HookItem`. `problem` is why it will not run as written. */
export type HookItem = {
  event: string;
  matcher: string;
  command: string;
  timeoutSecs: number;
  problem: string | null;
};
export type HooksView = { path: string; text: string; hooks: HookItem[] };

export async function hooksConfig(): Promise<HooksView> {
  if (!inTauri()) return { path: "", text: "", hooks: [] };
  return invoke<HooksView>("hooks_config_get");
}

/** Refused, and the file left alone, when the text is not a valid hooks config. */
export async function saveHooksConfig(text: string): Promise<HooksView> {
  requireBackend();
  return invoke<HooksView>("hooks_config_save", { text });
}
