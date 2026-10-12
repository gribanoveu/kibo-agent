import { useCallback, useEffect, useState } from "react";
import { catalogInstall, catalogList, catalogPreview, catalogRemove, type CatalogKind, type CatalogSource } from "../lib/catalog";

/** Which entry: its source's address, its kind and id — unique across the page. */
export type EntryKey = { source: string; kind: CatalogKind; id: string };

export const sameEntry = (a: EntryKey | null, b: EntryKey | null) =>
  a !== null && b !== null && a.source === b.source && a.kind === b.kind && a.id === b.id;

/**
 * The catalogs, asked when the page opens, on Refresh and after an install or
 * a remove — never on a timer: a catalog changes when someone publishes to it,
 * and asking costs a `304` only when it is asked.
 */
export function useCatalog() {
  const [sources, setSources] = useState<CatalogSource[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One action at a time: the entry it is on, and what the last one failed with.
  const [busy, setBusy] = useState<EntryKey | null>(null);
  const [failed, setFailed] = useState<{ entry: EntryKey; error: string } | null>(null);

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

  const act = async (entry: EntryKey, run: (source: string, kind: CatalogKind, id: string) => Promise<void>) => {
    setBusy(entry);
    setFailed(null);
    try {
      await run(entry.source, entry.kind, entry.id);
    } catch (e) {
      setFailed({ entry, error: String(e) });
    }
    setBusy(null);
    await refresh();
  };

  return {
    sources,
    loading,
    error,
    refresh,
    busy,
    failed,
    install: (entry: EntryKey) => act(entry, catalogInstall),
    remove: (entry: EntryKey) => act(entry, catalogRemove),
  };
}

/** The text of the entry a details panel shows, read when it is opened. */
export function useCatalogPreview(entry: EntryKey | null) {
  const [preview, setPreview] = useState<{ entry: EntryKey; text: string | null; error: string | null } | null>(null);
  const source = entry?.source;
  const kind = entry?.kind;
  const id = entry?.id;

  useEffect(() => {
    if (source === undefined || kind === undefined || id === undefined) return;
    const asked = { source, kind, id };
    let current = true;
    setPreview({ entry: asked, text: null, error: null });
    catalogPreview(source, kind, id).then(
      (text) => current && setPreview({ entry: asked, text, error: null }),
      (e) => current && setPreview({ entry: asked, text: null, error: String(e) }),
    );
    return () => {
      current = false;
    };
  }, [source, kind, id]);

  // A preview of the entry before is not this one's.
  return preview && sameEntry(preview.entry, entry) ? preview : null;
}
