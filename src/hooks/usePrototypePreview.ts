import { useEffect, useMemo, useRef, useState } from "react";
import type { Block } from "../lib/chatTurnReducer";
import { edits, pageUrl, versionsOf, type Presented } from "../lib/prototype";

/**
 * The Preview panel beside a Prototype chat (docs/31-prototype-role.md, P-5):
 * open or not, and what it shows — the working folder as it is now, or a
 * version presented earlier. Everything else comes from the transcript: the
 * versions are its `present` calls, and each edit of the folder it records is
 * a reason to draw the page again. No listener of its own — the turn's events
 * already arrive as blocks.
 *
 * `blocks` is `null` for a chat in any other role: no panel.
 */
export function usePrototypePreview(chatId: string | null, blocks: Block[] | null, running: boolean) {
  const versions = useMemo(() => (blocks ? versionsOf(blocks) : []), [blocks]);
  const changed = useMemo(() => (blocks ? edits(blocks) : 0), [blocks]);
  const [open, setOpen] = useState(false);
  // `null`: the working folder. A number: that version, as it was kept.
  const [shown, setShown] = useState<number | null>(null);
  // "Refresh", for a page that does something on load.
  const [refreshed, setRefreshed] = useState(0);
  // Over the chat's place too, as wide as the window allows.
  const [expanded, setExpanded] = useState(false);
  // A page's error the user handed to the composer; a new object each ask.
  const [fix, setFix] = useState<{ text: string } | null>(null);

  // A version presented in the turn under way opens the panel on the page as
  // it now is. Another chat starts closed: opening a saved one is for reading
  // it, and its cards open what they kept.
  const seen = useRef({ chatId, count: versions.length });
  useEffect(() => {
    const before = seen.current;
    seen.current = { chatId, count: versions.length };
    if (before.chatId !== chatId) {
      setOpen(false);
      setShown(null);
      setExpanded(false);
    } else if (running && versions.length > before.count) {
      setOpen(true);
      setShown(null);
    }
  }, [chatId, versions.length, running]);

  const latest: Presented | undefined = versions[versions.length - 1];
  const page = shown === null ? latest : versions.find((v) => v.version === shown);
  const url = chatId && page ? pageUrl(chatId, shown, page.path) : null;
  // The frame is drawn anew whenever this changes: the working folder with
  // every edit, a kept version only when asked.
  const key = `${url}#${shown === null ? changed : 0}#${refreshed}`;

  return {
    available: blocks !== null && versions.length > 0,
    open: open && url !== null,
    /** Taking the chat's place as well; the chat stays mounted under it. */
    expanded: open && url !== null && expanded,
    versions,
    shown,
    /** The page shown: its path and title. */
    page,
    url,
    key,
    /** Opens the panel on a version, or on the working folder with `null`. */
    show: (version: number | null) => {
      setShown(version);
      setOpen(true);
    },
    close: () => {
      setOpen(false);
      setExpanded(false);
    },
    toggle: () => {
      setOpen((was) => !was);
      setExpanded(false);
    },
    toggleExpanded: () => setExpanded((was) => !was),
    refresh: () => setRefreshed((n) => n + 1),
    fix,
    askFix: (text: string) => setFix({ text }),
  };
}

export type PrototypePreview = ReturnType<typeof usePrototypePreview>;
