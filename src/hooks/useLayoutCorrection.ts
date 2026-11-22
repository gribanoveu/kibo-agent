import { useCallback, useEffect, useRef, useState, type Dispatch, type RefObject, type SetStateAction } from "react";
import { countLayoutCorrection, detectLayoutCorrection, dominantLayout, type LayoutCorrection } from "../lib/layoutCorrection";
import { keyboardLayoutSet } from "../lib/keyboardLayout";

type Options = {
  on: boolean;
  text: string;
  setText: Dispatch<SetStateAction<string>>;
  area: RefObject<HTMLTextAreaElement | null>;
  draftKey: string;
  /** Also select the corrected prose's dominant system layout. */
  switchLayout?: boolean;
  onError?: (message: string) => void;
};

/** The agent box's wrong-layout hint. Detection is local and nothing changes until `apply`. */
export function useLayoutCorrection({ on, text, setText, area, draftKey, switchLayout = false, onError }: Options) {
  const [found, setSuggestion] = useState<{ source: string; result: LayoutCorrection } | null>(null);
  const lastShown = useRef<string | null>(null);
  const fallbackUndo = useRef<{ draft: string; source: string; corrected: string; start: number; end: number } | null>(null);
  // Until the debounce catches up, an old result is neither drawn nor counted for new text.
  const suggestion = found?.source === text ? found.result : null;

  const check = useCallback(() => {
    const result = on ? detectLayoutCorrection(text) : null;
    setSuggestion(result?.needsCorrection ? { source: text, result } : null);
  }, [on, text]);

  useEffect(() => {
    setSuggestion(null);
    if (!on) return;
    const timer = setTimeout(check, 500);
    return () => clearTimeout(timer);
  }, [check, draftKey, on]);

  useEffect(() => {
    const pending = fallbackUndo.current;
    if (pending && (pending.draft !== draftKey || pending.corrected !== text)) fallbackUndo.current = null;
  }, [draftKey, text]);

  useEffect(() => {
    if (!suggestion) {
      lastShown.current = null;
      return;
    }
    const key = `${text}\0${suggestion.corrected}`;
    if (lastShown.current !== key) countLayoutCorrection("shown");
    lastShown.current = key;
  }, [suggestion, text]);

  const apply = useCallback(() => {
    // Recheck: a debounced hint may belong to the keystroke just before this one.
    const result = on ? detectLayoutCorrection(text) : null;
    if (!result?.needsCorrection) {
      setSuggestion(null);
      return false;
    }
    const el = area.current;
    const start = el?.selectionStart ?? text.length;
    const end = el?.selectionEnd ?? start;
    setSuggestion(null);
    fallbackUndo.current = null;
    countLayoutCorrection("accepted");
    const layout = switchLayout ? dominantLayout(result.corrected) : null;
    if (layout) void keyboardLayoutSet(layout).catch((e) => onError?.(String(e)));

    if (!el) {
      fallbackUndo.current = { draft: draftKey, source: text, corrected: result.corrected, start, end };
      setText(result.corrected);
      return true;
    }
    el.focus();
    el.setSelectionRange(0, text.length);
    // A native insertion is one browser undo step; assigning React state alone is not.
    let inserted = false;
    try {
      inserted = typeof document.execCommand === "function" && document.execCommand("insertText", false, result.corrected);
    } catch {
      // The one-step snapshot below supplies Undo in older WebKit too.
    }
    if (!inserted) {
      fallbackUndo.current = { draft: draftKey, source: text, corrected: result.corrected, start, end };
      setText(result.corrected);
    }
    requestAnimationFrame(() => {
      el.setSelectionRange(start, end);
      el.focus();
    });
    return true;
  }, [area, draftKey, on, onError, setText, switchLayout, text]);

  const undo = useCallback(() => {
    const pending = fallbackUndo.current;
    if (!pending || pending.draft !== draftKey || pending.corrected !== text) return false;
    fallbackUndo.current = null;
    setText(pending.source);
    const el = area.current;
    requestAnimationFrame(() => {
      el?.setSelectionRange(pending.start, pending.end);
      el?.focus();
    });
    return true;
  }, [area, draftKey, setText, text]);

  return { suggestion, check, apply, undo };
}
