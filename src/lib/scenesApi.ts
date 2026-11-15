import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

/** A scene the model painted (`domain::scene::SceneInfo`); its body is read on its own. */
export type MadeScene = { id: string; title: string; created: number };

/** Every painted scene, newest first; none outside the app. */
export async function listScenes(): Promise<MadeScene[]> {
  if (!inTauri()) return [];
  return invoke<MadeScene[]>("scenes_list");
}

/** Scene `id`'s body: HTML and CSS for `sceneDocument`. */
export async function readScene(id: string): Promise<string> {
  requireBackend();
  return invoke<string>("scene_read", { id });
}

export async function deleteScene(id: string): Promise<void> {
  requireBackend();
  return invoke<void>("scene_delete", { id });
}
