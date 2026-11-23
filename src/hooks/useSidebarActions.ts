import type { useAgentTurn } from "./useAgentTurn";
import type { useChatGroups } from "./useChatGroups";
import type { useChatHistory } from "./useChatHistory";
import type { usePlainChat } from "./usePlainChat";

/** Why the open chat cannot be left: its turn would land in the next one. */
export const STAY = "Stop the running turn before leaving this chat";

/**
 * The sidebar's chat and group actions, in either mode: Agent's go through
 * the history store — a turn may be writing a row — and Chat's through its
 * own conversations. A refusal is said as a toast; a group's refusal comes
 * back as `error` for the dialog to show.
 */
export function useSidebarActions({
  agentMode,
  agent,
  plain,
  history,
  groups,
  notify,
  dropDraft,
}: {
  agentMode: boolean;
  agent: Pick<ReturnType<typeof useAgentTurn>, "discard">;
  plain: Pick<ReturnType<typeof usePlainChat>, "archive" | "pin" | "move" | "remove" | "refresh">;
  history: Pick<ReturnType<typeof useChatHistory>, "archive" | "pin" | "move" | "remove" | "refresh">;
  groups: Pick<ReturnType<typeof useChatGroups>, "create" | "rename" | "remove">;
  notify: (msg: string) => void;
  /** A deleted chat takes its unsent message with it. */
  dropDraft: (mode: "agent" | "chat", id: string) => void;
}) {
  const onArchiveChat = (id: string, archived: boolean) =>
    agentMode ? history.archive(id, archived).catch((e) => notify(String(e))) : void plain.archive(id, archived);
  const onPinChat = (id: string, pinned: boolean) =>
    agentMode ? history.pin(id, pinned).catch((e) => notify(String(e))) : void plain.pin(id, pinned);
  const onMoveChat = (id: string, group: string | null) =>
    agentMode ? history.move(id, group).catch((e) => notify(String(e))) : void plain.move(id, group);
  const onCreateGroup = async (name: string) => {
    try {
      return { id: (await groups.create(name)).id };
    } catch (e) {
      return { error: String(e) };
    }
  };
  const onRenameGroup = async (id: string, name: string) => {
    try {
      await groups.rename(id, name);
      return {};
    } catch (e) {
      return { error: String(e) };
    }
  };
  const onDeleteGroup = (id: string) =>
    void groups.remove(id).then(
      () => (agentMode ? history.refresh() : plain.refresh()),
      (e) => notify(String(e)),
    );
  const onDeleteChat = (id: string) => {
    if (!agentMode)
      return void plain.remove(id).then(
        (removed) => (removed ? dropDraft("chat", id) : notify(STAY)),
        (e) => notify(String(e)),
      );
    // Deleting first would let a save already in flight recreate the row.
    void agent.discard(id).then(
      (discarded) => {
        if (!discarded) return notify("Stop the running turn before deleting this chat");
        history.remove(id).then(
          () => dropDraft("agent", id),
          (e) => notify(String(e)),
        );
      },
      // A write behind the discard failed: say so, rather than swallow the delete.
      (e) => notify(String(e)),
    );
  };
  return { onArchiveChat, onPinChat, onMoveChat, onCreateGroup, onRenameGroup, onDeleteGroup, onDeleteChat };
}