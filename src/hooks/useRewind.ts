import { useCallback, useState } from "react";
import type { FileRewind } from "../lib/chat";

type Deps = {
  preview: (bubbleId: string) => Promise<{ files: FileRewind[]; unrecorded: number } | null>;
  rewind: (bubbleId: string) => Promise<FileRewind[] | null>;
  notify: (message: string) => void;
};

export type RewindAsk = { bubbleId: string; files: FileRewind[]; unrecorded: number };

/**
 * Rewinding the chat to before a message: what it would do is read and shown
 * first — the conversation after that point goes for good, so it is always
 * asked, even with no files to put back.
 */
export function useRewind({ preview, rewind, notify }: Deps) {
  const [asked, setAsked] = useState<RewindAsk | null>(null);

  const ask = useCallback(
    async (bubbleId: string) => {
      try {
        const found = await preview(bubbleId);
        if (found) setAsked({ bubbleId, ...found });
      } catch (e) {
        notify(String(e));
      }
    },
    [preview, notify],
  );

  const close = useCallback(() => setAsked(null), []);

  const confirm = useCallback(async () => {
    if (!asked) return;
    setAsked(null);
    await rewind(asked.bubbleId);
  }, [asked, rewind]);

  return { asked, ask, close, confirm };
}
