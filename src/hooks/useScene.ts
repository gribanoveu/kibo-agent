import { useMemo } from "react";
import { BUILTIN_SCENES } from "../lib/scenes";
import { isBoolean, useStoredState } from "./useStoredState";

/**
 * The scene behind Frost's glass (`docs/55-frost-scenes.md`): one the app
 * ships, one the model made, or none.
 */
export type SceneChoice = { kind: "builtin" | "made"; id: string } | null;

const isSceneChoice = (value: unknown): value is SceneChoice => {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const { kind, id } = value as Record<string, unknown>;
  return (kind === "builtin" || kind === "made") && typeof id === "string";
};

export const sameScene = (a: SceneChoice, b: SceneChoice) => a?.kind === b?.kind && a?.id === b?.id;

/**
 * The chosen scene and its body, and whether it keeps moving while the window
 * is in the background. A choice that names nothing there is now — a scene
 * since deleted — is no scene: `active` is null and nothing is drawn.
 */
export function useScene() {
  const [choice, choose] = useStoredState<SceneChoice>("kibo-scene", null, isSceneChoice);
  const [animateInBackground, setAnimateInBackground] = useStoredState("kibo-scene-animate-in-background", false, isBoolean);

  const shown = useMemo(() => {
    const builtin = choice?.kind === "builtin" ? BUILTIN_SCENES.find((s) => s.id === choice.id) : undefined;
    return builtin ? { active: choice, body: builtin.body() } : { active: null, body: null };
  }, [choice]);

  return { ...shown, choose, animateInBackground, setAnimateInBackground };
}
