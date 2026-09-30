import { useEffect, useState } from "react";
import { kubeRunbooks, type Runbook } from "../lib/kube";

/** Settings → Kubernetes → Runbooks: read when the pane opens. */
export function useKubeRunbooks() {
  const [listed, setListed] = useState<{ dir: string; runbooks: Runbook[] }>({ dir: "", runbooks: [] });
  useEffect(() => {
    kubeRunbooks().then(setListed, () => {});
  }, []);
  return listed;
}
