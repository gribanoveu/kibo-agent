import { useCallback, useEffect, useState } from "react";
import { type ChatSummary, deleteChat, listChats, setChatArchived } from "../lib/savedChats";

/**
 * The sidebar's list of saved conversations.
 *
 * Reloaded rather than kept in step: the backend scopes the list to the open
 * folder and orders it, and a copy of those two rules here is a copy that can
 * disagree. Refreshed when the workspace changes and when a turn has just
 * been written.
 */
export function useChatHistory(workspace: string | null) {
  const [chats, setChats] = useState<ChatSummary[]>([]);

  const refresh = useCallback(() => {
    listChats()
      .then(setChats)
      // A list that fails to load leaves the previous rows rather than
      // blanking the sidebar; nothing here is worth a message.
      .catch(() => {});
  }, []);

  useEffect(refresh, [workspace, refresh]);

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

  return { chats, refresh, remove, archive };
}
