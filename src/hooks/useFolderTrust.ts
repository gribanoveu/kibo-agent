import { useCallback, useEffect, useState } from "react";
import { folderTrust, setFolderTrust, type FolderTrust } from "../lib/chat";

/**
 * Whether the open folder is trusted with its own skills and `/` commands,
 * read each time another folder opens. Asks on its own only about a folder
 * that has some and was never asked about; `ask` opens the question again.
 * `onDecided` runs after an answer is saved — what reads the folder's
 * commands reads again.
 */
export function useFolderTrust(workspace: string | null, onDecided: () => void) {
  const [trust, setTrust] = useState<FolderTrust | null>(null);
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setAsking(false);
    setError(null);
    folderTrust().then(
      (read) => {
        if (!live) return;
        setTrust(read);
        setAsking(!!read?.needed && read.trusted === null);
      },
      () => live && setTrust(null),
    );
    return () => {
      live = false;
    };
  }, [workspace]);

  const decide = useCallback(
    async (trusted: boolean) => {
      try {
        await setFolderTrust(trusted);
        setTrust((read) => read && { ...read, trusted });
        setAsking(false);
        onDecided();
      } catch (e) {
        setError(String(e));
      }
    },
    [onDecided],
  );

  return {
    trust,
    asking,
    error,
    ask: useCallback(() => setAsking(true), []),
    close: useCallback(() => setAsking(false), []),
    decide,
  };
}
