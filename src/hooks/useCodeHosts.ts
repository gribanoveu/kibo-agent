import { useCallback, useEffect, useState } from "react";
import { codeHostDelete, codeHostDetect, codeHosts, codeHostSave, type CodeHostServer } from "../lib/pullRequests";

/**
 * The Bitbucket servers the user added, for the Bitbucket feature's panel
 * in Catalog: read when it opens; a save or a delete answers with the list after it. Nothing else
 * changes them, so there is nothing to listen to.
 */
export function useCodeHosts() {
  const [servers, setServers] = useState<CodeHostServer[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    codeHosts().then(
      (list) => live && setServers(list),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
    };
  }, []);

  /** Throws for the form to show; the list is kept as it was. */
  const save = useCallback(async (server: CodeHostServer, token: string | null) => {
    setServers(await codeHostSave(server, token, false));
    setError(null);
  }, []);

  const remove = useCallback(async (id: string) => {
    try {
      setServers(await codeHostDelete(id));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  return { servers, error, save, remove, detect: codeHostDetect };
}
