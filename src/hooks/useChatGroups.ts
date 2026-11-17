import { useCallback, useEffect, useRef, useState } from "react";
import { type ChatGroup, createChatGroup, deleteChatGroup, listChatGroups, renameChatGroup } from "../lib/savedChats";

const NONE: ChatGroup[] = [];

/**
 * The sidebar's chat folders, one list per open folder (F-8.6) — `null` is
 * Chat mode's shared one. Read with the chat lists and re-read on the same
 * triggers — the backend reports no event for folders, and the list's own
 * refresh is the signal.
 */
export function useChatGroups(scopes: (string | null)[]) {
  const [byScope, setByScope] = useState<Record<string, ChatGroup[]>>({});
  // A scope's key: its folder, "" for Chat mode's set. Joined so the effect
  // runs when the set of folders changes, not on every new array.
  const key = JSON.stringify(scopes);
  // As in `useChatHistory`: a read that started before the scopes changed
  // lands into the scopes of now.
  const current = useRef(key);
  current.current = key;

  const refresh = useCallback(() => {
    const listed = JSON.parse(key) as (string | null)[];
    void Promise.all(
      listed.map((scope) =>
        listChatGroups(scope).then(
          (groups) => [scope ?? "", groups] as const,
          // A failed read leaves the previous folders rather than a blank list.
          () => [scope ?? "", null] as const,
        ),
      ),
    ).then((lists) => {
      const read = new Map(lists);
      const now = (JSON.parse(current.current) as (string | null)[]).map((scope) => scope ?? "");
      setByScope((shown) => Object.fromEntries(now.map((scope) => [scope, read.get(scope) ?? shown[scope] ?? NONE])));
    });
  }, [key]);

  useEffect(refresh, [refresh]);

  const groupsOf = useCallback((scope: string | null) => byScope[scope ?? ""] ?? NONE, [byScope]);

  const create = useCallback(
    async (scope: string | null, name: string) => {
      const made = await createChatGroup(scope, name);
      refresh();
      return made;
    },
    [refresh],
  );

  const rename = useCallback(
    async (scope: string | null, id: string, name: string) => {
      await renameChatGroup(scope, id, name);
      refresh();
    },
    [refresh],
  );

  const remove = useCallback(
    async (scope: string | null, id: string) => {
      // The chats of the removed folder stay, now unfiled — the chat list
      // re-reads on the caller's side.
      await deleteChatGroup(scope, id);
      refresh();
    },
    [refresh],
  );

  return { groupsOf, refresh, create, rename, remove };
}
