import { useCallback, useEffect, useState } from "react";
import { catalogInstall, catalogList, catalogPreview, catalogRemove, catalogUpdate, catalogUpdatePreview, type CatalogKind, type CatalogSource } from "../lib/catalog";

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
    /** `secrets`: a server's placeholders, by name. */
    install: (entry: EntryKey, secrets: Record<string, string> = {}) => act(entry, (source, kind, id) => catalogInstall(source, kind, id, secrets)),
    remove: (entry: EntryKey) => act(entry, catalogRemove),
    update: (entry: EntryKey, secrets: Record<string, string> = {}) => act(entry, (source, kind, id) => catalogUpdate(source, kind, id, secrets)),
  };
}

/** The entry a details panel or the install dialog shows, read when it is opened. */
export const useCatalogPreview = (entry: EntryKey | null) => useEntryRead(entry, catalogPreview);

/** What updating the entry would change, read when its dialog opens. */
export const useCatalogUpdatePreview = (entry: EntryKey | null) => useEntryRead(entry, catalogUpdatePreview);

/** `ask` of the entry, asked again when the entry changes; an answer about the one before is dropped. */
function useEntryRead<T>(entry: EntryKey | null, ask: (source: string, kind: CatalogKind, id: string) => Promise<T>) {
  const [answer, setAnswer] = useState<{ entry: EntryKey; read: T | null; error: string | null } | null>(null);
  const source = entry?.source;
  const kind = entry?.kind;
  const id = entry?.id;

  useEffect(() => {
    if (source === undefined || kind === undefined || id === undefined) return;
    const asked = { source, kind, id };
    let current = true;
    setAnswer({ entry: asked, read: null, error: null });
    ask(source, kind, id).then(
      (read) => current && setAnswer({ entry: asked, read, error: null }),
      (e) => current && setAnswer({ entry: asked, read: null, error: String(e) }),
    );
    return () => {
      current = false;
    };
  }, [source, kind, id, ask]);

  return answer && sameEntry(answer.entry, entry) ? answer : null;
}
