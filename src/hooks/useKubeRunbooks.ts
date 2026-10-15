import { useEffect, useState } from "react";
import { kubeRunbooks, type Runbook } from "../lib/kube";

/** Settings → Kubernetes → Runbooks: read when the pane opens; `error` says why they could not be. */
export function useKubeRunbooks() {
  const [listed, setListed] = useState<{ dir: string; runbooks: Runbook[] }>({ dir: "", runbooks: [] });
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    kubeRunbooks().then(setListed, (e) => setError(String(e)));
  }, []);
  return { ...listed, error };
}
