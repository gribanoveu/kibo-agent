import { useCallback, useEffect, useState } from "react";
import { pickSavePath } from "../lib/dialog";
import { addHttpCert, deleteHttpCert, exportHttpCert, listHttpCerts, type TrustedCert } from "../lib/httpCommands";

/**
 * An environment's certificates, a file each on the backend. Only the form changes
 * them, so it reads the list again after each change rather than listening.
 * Each action throws what the backend refused.
 */
export function useHttpCerts(environment: string) {
  const [certs, setCerts] = useState<TrustedCert[]>([]);
  const [failed, setFailed] = useState<string | null>(null);

  const reload = useCallback(
    () =>
      listHttpCerts(environment).then(
        (listed) => {
          setCerts(listed);
          setFailed(null);
        },
        (e) => setFailed(`The certificates were not read: ${e}`),
      ),
    [environment],
  );
  useEffect(() => {
    reload();
  }, [reload]);

  /** `pem` kept under `name`, numbered if taken; the name it got. */
  const add = useCallback(
    async (name: string, pem: string) => {
      const named = await addHttpCert(environment, name, pem);
      await reload();
      return named;
    },
    [environment, reload],
  );

  const remove = useCallback(
    async (name: string) => {
      await deleteHttpCert(environment, name);
      await reload();
    },
    [environment, reload],
  );

  /** The file where the save dialog says; false when cancelled. */
  const exportCert = useCallback(
    async (name: string) => {
      const dest = await pickSavePath(name, "pem");
      if (!dest) return false;
      await exportHttpCert(environment, name, dest);
      return true;
    },
    [environment],
  );

  return { certs, failed, add, remove, exportCert };
}
