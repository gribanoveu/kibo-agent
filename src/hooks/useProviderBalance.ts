import { useEffect, useState } from "react";
import { onFeaturesChanged } from "../lib/features";
import { providerBalance, type ProviderBalance } from "../lib/providerBalance";

/**
 * What is left on `providerId`'s account: read when the provider changes, when
 * a feature is turned on or off, and when a turn ends — an answer is what
 * spends it. Nothing is read while a turn runs; the last value stays.
 */
export function useProviderBalance(providerId: string | null, running: boolean) {
  const [balance, setBalance] = useState<ProviderBalance | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [switched, setSwitched] = useState(0);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | undefined;
    onFeaturesChanged(() => setSwitched((n) => n + 1)).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (running) return;
    if (!providerId) {
      setBalance(null);
      setError(null);
      return;
    }
    let live = true;
    providerBalance(providerId).then(
      (next) => {
        if (!live) return;
        setBalance(next);
        setError(null);
      },
      (e) => {
        if (!live) return;
        setBalance(null);
        setError(String(e));
      },
    );
    return () => {
      live = false;
    };
  }, [providerId, running, switched]);

  return { balance, error };
}
