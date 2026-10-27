import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./backend";
import { forgetHistory } from "./mongo";

/** A feature built into the app that the user turns on from the Catalog
    (`domain::features`, docs/33-features.md). Its id is what a page's `feature` names. */
export type FeatureView = {
  id: string;
  name: string;
  description: string;
  on: boolean;
  /** What the model is given, as it is told. */
  tools: { name: string; description: string }[];
  /** Where: "Assistant chat", "Agent", "Plan", "Ask". */
  places: string[];
  /** Its file, `~/.kibo/…`; null for a feature with none. */
  data: string | null;
  hasData: boolean;
};

export const FEATURES_EVENT = "features:changed";

export async function onFeaturesChanged(handler: () => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen(FEATURES_EVENT, () => handler());
}

export async function featuresList(): Promise<FeatureView[]> {
  if (!inTauri()) return [];
  return invoke<FeatureView[]>("features_list");
}

const requireBackend = () => {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
};

/** From the next turn on. The list after it. */
export async function setFeature(id: string, on: boolean): Promise<FeatureView[]> {
  requireBackend();
  return invoke<FeatureView[]>("features_set", { id, on });
}

/** Only while it is off. The list after it. */
export async function deleteFeatureData(id: string): Promise<FeatureView[]> {
  requireBackend();
  const list = await invoke<FeatureView[]>("features_delete_data", { id });
  // What the MongoDB page keeps in this window's storage goes with its connections.
  if (id === "mongodb") forgetHistory();
  return list;
}
