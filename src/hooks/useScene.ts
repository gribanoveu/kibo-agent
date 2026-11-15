import { useCallback, useEffect, useMemo, useState } from "react";
import { BUILTIN_SCENES } from "../lib/scenes";
import { deleteScene, listScenes, readScene, type MadeScene } from "../lib/scenesApi";
import { isBoolean, useStoredState } from "./useStoredState";

/**
 * The scene behind Frost's glass (`docs/55-frost-scenes.md`): one the app
 * ships, one the model made, or none.
 */
export type SceneChoice = { kind: "builtin" | "made"; id: string } | null;

/** A painted scene with its body, as the Generated group draws it. */
export type MadeSceneBody = MadeScene & { body: string };

const isSceneChoice = (value: unknown): value is SceneChoice => {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const { kind, id } = value as Record<string, unknown>;
  return (kind === "builtin" || kind === "made") && typeof id === "string";
};

export const sameScene = (a: SceneChoice, b: SceneChoice) => a?.kind === b?.kind && a?.id === b?.id;

/**
 * Every painted scene with its body; one whose file cannot be read is left
 * out rather than failing the rest.
 */
// ponytail: every body read up front — 50 at 256 KB at most; read a card's body when it shows if that is felt.
async function readMade(): Promise<MadeSceneBody[]> {
  const rows = await listScenes();
  const read = await Promise.all(rows.map((row) => readScene(row.id).then((body) => ({ ...row, body }), () => null)));
  return read.filter((scene): scene is MadeSceneBody => scene !== null);
}

/**
 * The chosen scene and its body, the painted ones, and whether the scene keeps
 * moving while the window is in the background. A choice that names nothing
 * there is now — a scene since deleted — is no scene: `active` is null and
 * nothing is drawn.
 */
export function useScene() {
  const [choice, choose] = useStoredState<SceneChoice>("kibo-scene", null, isSceneChoice);
  const [animateInBackground, setAnimateInBackground] = useStoredState("kibo-scene-animate-in-background", false, isBoolean);
  const [made, setMade] = useState<MadeSceneBody[]>([]);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(
    () =>
      readMade().then(
        (next) => {
          setMade(next);
          setError(null);
        },
        (e) => setError(String(e)),
      ),
    [],
  );
  useEffect(() => void reload(), [reload]);

  const remove = useCallback(
    async (id: string) => {
      try {
        await deleteScene(id);
      } catch (e) {
        setError(String(e));
        return;
      }
      await reload();
    },
    [reload],
  );

  const shown = useMemo(() => {
    const body =
      choice?.kind === "builtin"
        ? BUILTIN_SCENES.find((s) => s.id === choice.id)?.body()
        : choice?.kind === "made"
          ? made.find((s) => s.id === choice.id)?.body
          : undefined;
    return body === undefined ? { active: null, body: null } : { active: choice, body };
  }, [choice, made]);

  return { ...shown, choose, made, remove, error, animateInBackground, setAnimateInBackground };
}
