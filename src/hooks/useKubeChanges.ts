import { useEffect, useState } from "react";
import { kubeChanges, type KubeChange } from "../lib/kube";

/** Settings → Kubernetes → Changes: read when the pane opens. */
export function useKubeChanges() {
  const [changes, setChanges] = useState<KubeChange[]>([]);
  useEffect(() => {
    kubeChanges().then(setChanges, () => setChanges([]));
  }, []);
  return changes;
}
