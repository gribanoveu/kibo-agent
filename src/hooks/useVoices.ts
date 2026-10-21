import { useEffect, useState } from "react";
import { meetingVoiceDelete, meetingVoices, type VoiceInfo } from "../lib/meeting";

/**
 * The voice memory's voices, read when the list opens and again whenever
 * `changed` does — a rename or a recording can teach it one.
 */
export function useVoices(changed: unknown) {
  const [voices, setVoices] = useState<VoiceInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    meetingVoices().then(
      (next) => live && setVoices(next),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
    };
  }, [changed]);

  const remove = async (id: string) => {
    try {
      setVoices(await meetingVoiceDelete(id));
    } catch (e) {
      setError(String(e));
    }
  };

  return { voices, error, clearError: () => setError(null), remove };
}
