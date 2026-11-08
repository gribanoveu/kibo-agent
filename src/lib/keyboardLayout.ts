import { invoke } from "@tauri-apps/api/core";

export type KeyboardLayout = "ru" | "en";

/** Select an already-enabled system layout; only after the user explicitly applies Fix. */
export async function keyboardLayoutSet(layout: KeyboardLayout): Promise<void> {
  return invoke("keyboard_layout_set", { layout });
}
