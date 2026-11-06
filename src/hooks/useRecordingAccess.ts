import { useCallback, useEffect, useState } from "react";
import { meetingRecordingAccess, meetingRecordingAccessRequest, meetingRecordingAccessSettings, type Access, type RecordingAccess, type Source } from "../lib/meeting";

/**
 * Whether the system lets the app record each side of a call, read when the
 * page opens and again whenever the window comes back to the front — System
 * Settings, where a refused one is turned on, is another window. `allow` asks
 * with the system's prompt, or opens System Settings for one refused before:
 * no prompt asks that again; it resolves with the system's answer, when a prompt
 * gave one. `ready` once nothing is missing — Record waits for it. Off, nothing is asked.
 */
export function useRecordingAccess(on = true) {
  const [access, setAccess] = useState<RecordingAccess | null>(null);
  const [error, setError] = useState<string | null>(null);
  const read = useCallback(() => void meetingRecordingAccess().then(setAccess, (e) => setError(String(e))), []);

  useEffect(() => {
    if (!on) return;
    read();
    window.addEventListener("focus", read);
    return () => window.removeEventListener("focus", read);
  }, [read, on]);

  const allow = async (source: Source): Promise<Access | undefined> => {
    setError(null);
    try {
      if (access?.[source] === "denied") await meetingRecordingAccessSettings(source);
      else {
        const now = await meetingRecordingAccessRequest(source);
        setAccess((was) => was && { ...was, [source]: now });
        return now;
      }
    } catch (e) {
      setError(String(e));
    }
  };

  // Both sides allowed, or the system does not say: a recording started without
  // one would ask for it there, past the note that says what is missing.
  const ready = access !== null && Object.values(access).every((a) => a === "granted" || a === "unknown");

  return { access, ready, error, clearError: () => setError(null), allow };
}
