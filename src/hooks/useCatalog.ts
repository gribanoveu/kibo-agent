import { useCallback, useEffect, useState } from "react";
import { catalogList, type CatalogSource } from "../lib/catalog";

/**
 * The catalogs, asked when the page opens and on Refresh — never on a timer:
 * a catalog changes when someone publishes to it, and asking costs a `304`
 * only when it is asked.
 */
export function useCatalog() {
  const [sources, setSources] = useState<CatalogSource[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setSources(await catalogList());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { sources, loading, error, refresh };
}
