import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type ChatSummary, deleteChat, listChats, setChatArchived, setChatGroup, setChatPinned } from "../lib/savedChats";

const NONE: ChatSummary[] = [];

/**
 * The sidebar's saved conversations, a list per open folder (F-8.6), and the
 * active folder's on its own — what the titlebar and the IDE's switcher name.
 *
 * Reloaded rather than kept in step: the backend scopes each list to its
 * folder and orders it, and a copy of those two rules here is a copy that
 * can disagree. Refreshed when the open folders change and when a turn has
 * just been written — every list, since a save names no folder here.
 */
export function useChatHistory(folders: string[], active: string | null) {
  const [byFolder, setByFolder] = useState<Record<string, ChatSummary[]>>({});
  // The active folder is listed even in the moment before the open set
  // catches up with it.
  const listed = active && !folders.includes(active) ? [active, ...folders] : folders;
  const key = listed.join("\n");
  // The folders listed now: a refresh that started before a folder opened or
  // closed lands with the lists of then, and must neither drop the new one
  // nor bring the closed one back.
  const current = useRef(key);
  current.current = key;

  const refresh = useCallback(() => {
    const paths = key ? key.split("\n") : [];
    void Promise.all(
      paths.map((path) =>
        listChats(path).then(
          (chats) => [path, chats] as const,
          // A list that fails to load keeps its previous rows rather than
          // blanking its section; nothing here is worth a message.
          () => [path, null] as const,
        ),
      ),
    ).then((lists) => {
      const read = new Map(lists);
      const now = current.current ? current.current.split("\n") : [];
      setByFolder((shown) => Object.fromEntries(now.map((path) => [path, read.get(path) ?? shown[path] ?? NONE])));
    });
  }, [key]);

  useEffect(refresh, [refresh]);

  const chats = (active && byFolder[active]) || NONE;

  const remove = useCallback(
    async (id: string) => {
      await deleteChat(id);
      refresh();
    },
    [refresh],
  );

  const archive = useCallback(
    async (id: string, archived: boolean) => {
      await setChatArchived(id, archived);
      refresh();
    },
    [refresh],
  );

  const pin = useCallback(
    async (id: string, pinned: boolean) => {
      await setChatPinned(id, pinned);
      refresh();
    },
    [refresh],
  );

  const move = useCallback(
    async (id: string, groupId: string | null) => {
      await setChatGroup(id, groupId);
      refresh();
    },
    [refresh],
  );

  /** The folder a chat is listed under — where opening it makes active. */
  const folderOf = useMemo(() => {
    const owner = new Map<string, string>();
    for (const [path, list] of Object.entries(byFolder)) for (const chat of list) owner.set(chat.id, path);
    return (id: string) => owner.get(id) ?? null;
  }, [byFolder]);

  return { chats, byFolder, folderOf, refresh, remove, archive, pin, move };
}
