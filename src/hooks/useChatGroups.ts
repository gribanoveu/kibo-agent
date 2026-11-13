import { useCallback, useEffect, useState } from "react";
import { type ChatGroup, createChatGroup, deleteChatGroup, listChatGroups, renameChatGroup } from "../lib/savedChats";

/**
 * The sidebar's chat folders, one list per open folder (Chat mode shares
 * one). Read with the chat list and re-read on the same triggers — the
 * backend reports no event for folders, and the list's own refresh is the
 * signal (docs/54-chat-groups.md).
 */
export function useChatGroups(workspace: string | null) {
  const [groups, setGroups] = useState<ChatGroup[]>([]);

  const refresh = useCallback(() => {
    listChatGroups(workspace)
      .then(setGroups)
      // A failed read leaves the previous folders rather than a blank list.
      .catch(() => {});
  }, [workspace]);

  useEffect(refresh, [workspace, refresh]);

  const create = useCallback(
    async (name: string) => {
      const made = await createChatGroup(workspace, name);
      refresh();
      return made;
    },
    [workspace, refresh],
  );

  const rename = useCallback(
    async (id: string, name: string) => {
      await renameChatGroup(workspace, id, name);
      refresh();
    },
    [workspace, refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      // The chats of the removed folder stay, now unfiled — the chat list
      // re-reads on the caller's side.
      await deleteChatGroup(workspace, id);
      refresh();
    },
    [workspace, refresh],
  );

  return { groups, refresh, create, rename, remove };
}
