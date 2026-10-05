import { useCallback, useState } from "react";
import type { FileRewind } from "../lib/chat";

type Deps = {
  preview: (bubbleId: string) => Promise<{ files: FileRewind[]; unrecorded: number } | null>;
  rewind: (bubbleId: string, summarize?: boolean) => Promise<FileRewind[] | null>;
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
  // A rewind with a summary waits for the model; the question stays up meanwhile.
  const [summarizing, setSummarizing] = useState(false);

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

  const close = useCallback(() => {
    if (!summarizing) setAsked(null);
  }, [summarizing]);

  const confirm = useCallback(
    async (summarize = false) => {
      if (!asked || summarizing) return;
      if (!summarize) {
        setAsked(null);
        await rewind(asked.bubbleId);
        return;
      }
      setSummarizing(true);
      try {
        await rewind(asked.bubbleId, true);
      } finally {
        setSummarizing(false);
        setAsked(null);
      }
    },
    [asked, summarizing, rewind],
  );

  return { asked, summarizing, ask, close, confirm };
}
