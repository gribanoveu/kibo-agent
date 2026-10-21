import { useCallback, useEffect, useState } from "react";
import { meetingRecordingAccess, meetingRecordingAccessRequest, meetingRecordingAccessSettings, type RecordingAccess, type Source } from "../lib/meeting";

/**
 * Whether the system lets the app record each side of a call, read when the
 * page opens and again whenever the window comes back to the front — System
 * Settings, where a refused one is turned on, is another window. `allow` asks
 * with the system's prompt, or opens System Settings for one refused before:
 * no prompt asks that again. `ready` once nothing is missing — Record waits for it.
 */
export function useRecordingAccess() {
  const [access, setAccess] = useState<RecordingAccess | null>(null);
  const [error, setError] = useState<string | null>(null);
  const read = useCallback(() => void meetingRecordingAccess().then(setAccess, (e) => setError(String(e))), []);

  useEffect(() => {
    read();
    window.addEventListener("focus", read);
    return () => window.removeEventListener("focus", read);
  }, [read]);

  const allow = async (source: Source) => {
    setError(null);
    try {
      if (access?.[source] === "denied") await meetingRecordingAccessSettings(source);
      else {
        const now = await meetingRecordingAccessRequest(source);
        setAccess((was) => was && { ...was, [source]: now });
      }
    } catch (e) {
      setError(String(e));
    }
  };

  // Both sides allowed, or the system does not say: a recording started without
  // one would ask for it there, past the note that says what is missing.
  const ready = access !== null && Object.values(access).every((a) => a === "granted" || a === "unknown");

  return { access, ready, error, allow };
}
