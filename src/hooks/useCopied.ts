import { useEffect, useRef, useState } from "react";

/**
 * Copies text and says so for a moment: `copied` is true for 1.5 s after a
 * copy that worked. A clipboard that refuses loses nothing — the text is on
 * screen — so a failure only leaves the button as it was.
 */
export function useCopied(): { copied: boolean; copy: (text: string) => void } {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = (text: string) => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1500);
    }, () => {});
  };
  return { copied, copy };
}
