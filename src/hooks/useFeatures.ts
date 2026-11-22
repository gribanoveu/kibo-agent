import { useCallback } from "react";
import { useLiveRead } from "./useLiveRead";
import { deleteFeatureData, featuresList, onFeaturesChanged, setFeature, setFeatureLayoutSwitch, setFeatureTools, type FeatureView } from "../lib/features";

/**
 * The features the Catalog turns on: read once, and again whenever one is
 * turned on or off — the sidebar and the Catalog page each hold one, and a
 * switch in either reaches both through the event.
 */
export function useFeatures() {
  const { value: features, setValue: setFeatures, error, setError } = useLiveRead(featuresList, onFeaturesChanged, [] as FeatureView[]);

  const act = useCallback(async (run: () => Promise<FeatureView[]>) => {
    try {
      setFeatures(await run());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const set = useCallback((id: string, on: boolean) => act(() => setFeature(id, on)), [act]);
  const setTools = useCallback((id: string, on: boolean) => act(() => setFeatureTools(id, on)), [act]);
  const setLayoutSwitch = useCallback((on: boolean) => act(() => setFeatureLayoutSwitch(on)), [act]);
  const deleteData = useCallback((id: string) => act(() => deleteFeatureData(id)), [act]);
  const isOn = useCallback((id: string) => features.some((f) => f.id === id && f.on), [features]);
  const layoutSwitch = features.some((f) => f.id === "layout-correction" && f.layoutSwitchOn && f.layoutSwitchSupported);

  return { features, error, set, setTools, setLayoutSwitch, deleteData, isOn, layoutSwitch };
}
