import { useEffect, useState } from "react";
import type { PendingToolCall, ToolPreview } from "../lib/chat";

/**
 * What each of a paused round's calls would do, asked of the backend once per
 * round — approving a write means approving its contents, and the arguments
 * alone do not show them. `error` says why there are none: an approval card
 * without its diff has to say it is without one, not look like a call that
 * changes nothing.
 */
export function useToolPreviews(
  calls: PendingToolCall[],
  preview: (calls: PendingToolCall[]) => Promise<ToolPreview[]>,
): { previews: ToolPreview[]; error: string | null } {
  const [previews, setPreviews] = useState<ToolPreview[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setError(null);
    preview(calls).then(
      // `?? []`: the card is worth drawing even if the previews are not.
      (next) => live && setPreviews(next ?? []),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
    };
  }, [calls, preview]);
  return { previews, error };
}
