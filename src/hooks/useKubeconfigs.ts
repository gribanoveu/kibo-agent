import { useCallback, useEffect, useState } from "react";
import {
  activeKubeconfig,
  kubeSettings,
  pickKubeconfig,
  removeKubeconfig,
  saveKubeconfig,
  type KubeSettings,
} from "../lib/kube";

/** The kubeconfig files of Settings → Kubernetes, and the one the chat's tab picked. */
export function useKubeconfigs() {
  const [settings, setSettings] = useState<KubeSettings | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setSettings(await kubeSettings());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  const run = useCallback(
    async (work: () => Promise<void>) => {
      setError(null);
      try {
        await work();
        await reload();
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      }
    },
    [reload],
  );

  return {
    configs: settings?.configs ?? [],
    active: activeKubeconfig(settings),
    error,
    save: useCallback((name: string, path: string) => run(() => saveKubeconfig(name, path)), [run]),
    remove: useCallback((name: string) => run(() => removeKubeconfig(name)), [run]),
    pick: useCallback((name: string) => run(() => pickKubeconfig(name)), [run]),
  };
}

export type KubeconfigsState = ReturnType<typeof useKubeconfigs>;
