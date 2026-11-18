import { useCallback, useEffect, useRef, useState } from "react";
import { type ChatGroup, type GroupList, createChatGroup, deleteChatGroup, listChatGroups, renameChatGroup } from "../lib/savedChats";

const NONE: ChatGroup[] = [];

/**
 * The sidebar's chat folders: Chat mode's set, or the one every open project
 * shares — a folder holds chats of several projects. Read with the chat lists
 * and re-read on the same triggers — the backend reports no event for
 * folders, and the list's own refresh is the signal.
 */
export function useChatGroups(list: GroupList) {
  const [read, setRead] = useState<{ list: GroupList; groups: ChatGroup[] } | null>(null);
  // A read that started before the mode changed is not the set on screen.
  const current = useRef(list);
  current.current = list;

  const refresh = useCallback(() => {
    void listChatGroups(list).then(
      (groups) => current.current === list && setRead({ list, groups }),
      // A failed read leaves the previous folders rather than a blank list.
      () => {},
    );
  }, [list]);

  useEffect(refresh, [refresh]);

  const groups = read?.list === list ? read.groups : NONE;

  const create = useCallback(
    async (name: string) => {
      const made = await createChatGroup(list, name);
      refresh();
      return made;
    },
    [list, refresh],
  );

  const rename = useCallback(
    async (id: string, name: string) => {
      await renameChatGroup(list, id, name);
      refresh();
    },
    [list, refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      // The chats of the removed folder stay, now unfiled — the chat list
      // re-reads on the caller's side.
      await deleteChatGroup(list, id);
      refresh();
    },
    [list, refresh],
  );

  return { groups, refresh, create, rename, remove };
}
