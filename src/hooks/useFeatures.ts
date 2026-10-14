import { useCallback, useEffect, useState } from "react";
import { deleteFeatureData, featuresList, onFeaturesChanged, setFeature, type FeatureView } from "../lib/features";

/**
 * The features the Catalog turns on: read once, and again whenever one is
 * turned on or off — the sidebar and the Catalog page each hold one, and a
 * switch in either reaches both through the event.
 */
export function useFeatures() {
  const [features, setFeatures] = useState<FeatureView[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const load = () =>
      featuresList().then(
        (list) => {
          if (!live) return;
          setFeatures(list);
          setError(null);
        },
        (e) => live && setError(String(e)),
      );
    load();
    let unlisten: (() => void) | undefined;
    onFeaturesChanged(() => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  const act = useCallback(async (run: () => Promise<FeatureView[]>) => {
    try {
      setFeatures(await run());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const set = useCallback((id: string, on: boolean) => act(() => setFeature(id, on)), [act]);
  const deleteData = useCallback((id: string) => act(() => deleteFeatureData(id)), [act]);
  const isOn = useCallback((id: string) => features.some((f) => f.id === id && f.on), [features]);

  return { features, error, set, deleteData, isOn };
}
