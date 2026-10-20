import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Block } from "../lib/chatTurnReducer";
import { prepareImageFile } from "../lib/images";
import {
  addPrototypeAsset,
  edits,
  pageUrl,
  prototypeAssets,
  removePrototypeAsset,
  versionsOf,
  type Assets,
  type Pointed,
  type Presented,
} from "../lib/prototype";

/**
 * The Preview panel beside a Prototype chat (docs/31-prototype-role.md, P-5):
 * open or not, and what it shows — the working folder as it is now, or a
 * version presented earlier. Everything else comes from the transcript: the
 * versions are its `present` calls, and each edit of the folder it records is
 * a reason to draw the page again. No listener of its own — the turn's events
 * already arrive as blocks. The pictures in the folder's `assets/` are read
 * again on the same signals, and when a turn ends.
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
  // The element the user pointed at, waiting over the message box to be sent with the next message.
  const [pointed, setPointed] = useState<Pointed | null>(null);
  // A page's error the user handed to the composer; a new object each ask.
  const [fix, setFix] = useState<{ text: string } | null>(null);
  // The pictures in `assets/`, and whether the drawer under the page is open.
  const [assets, setAssets] = useState<Assets | null>(null);
  const [assetsOpen, setAssetsOpen] = useState(false);
  const prototype = blocks !== null;

  // Only a Prototype chat that has an id has a folder to read.
  const reloadAssets = useCallback(async () => {
    if (!chatId || !prototype) return setAssets(null);
    try {
      setAssets(await prototypeAssets(chatId));
    } catch {
      setAssets(null);
    }
  }, [chatId, prototype]);
  useEffect(() => void reloadAssets(), [reloadAssets, changed, running]);

  // A version presented in the turn under way opens the panel on the page as
  // it now is. Another chat starts closed: opening a saved one is for reading
  // it, and its cards open what they kept.
  const seen = useRef({ chatId, count: versions.length });
  useEffect(() => {
    const before = seen.current;
    seen.current = { chatId, count: versions.length };
    // A new chat getting its id with its first message is the same chat.
    if (before.chatId !== chatId && before.chatId !== null) {
      setOpen(false);
      setShown(null);
      setExpanded(false);
      setPointed(null);
      setAssetsOpen(false);
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
  // Pictures are something to show before there is a page.
  const content = url !== null || (assets?.assets.length ?? 0) > 0;

  return {
    available: prototype && (versions.length > 0 || (assets?.assets.length ?? 0) > 0),
    open: open && content,
    /** Taking the chat's place as well; the chat stays mounted under it. */
    expanded: open && content && expanded,
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
    pointed,
    /** The element picked on the page, for the next message; `null` drops it. */
    point: setPointed,
    refresh: () => setRefreshed((n) => n + 1),
    fix,
    /** Puts `text` into the message being written, after what is there. */
    askFix: (text: string) => setFix({ text }),
    assets,
    assetsOpen,
    toggleAssets: () => setAssetsOpen((was) => !was),
    /** Opens the panel on its drawer: pictures were just put in the folder. */
    showAssets: () => {
      setOpen(true);
      setAssetsOpen(true);
      void reloadAssets();
    },
    reloadAssets,
    /**
     * Adds the picture files at `paths` to `assets/`, each sanitized as an
     * attachment is. The sentences for the ones that could not be added come
     * back; the rest are in.
     */
    addAssets: async (paths: string[]): Promise<string[]> => {
      if (!chatId) return [];
      const failed: string[] = [];
      for (const path of paths) {
        const name = path.split(/[\\/]/).pop() || path;
        try {
          await addPrototypeAsset(chatId, name, await prepareImageFile(path));
        } catch (e) {
          failed.push(`${name}: ${e}`);
        }
      }
      await reloadAssets();
      return failed;
    },
    removeAsset: async (name: string) => {
      if (!chatId) return;
      try {
        await removePrototypeAsset(chatId, name);
      } finally {
        await reloadAssets();
      }
    },
  };
}

export type PrototypePreview = ReturnType<typeof usePrototypePreview>;
