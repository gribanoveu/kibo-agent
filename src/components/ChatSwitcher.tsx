import { Dropdown } from "./Dropdown";
import { shortcutText } from "../lib/shortcuts";
import type { ChatSummary } from "../lib/chat";
import "./ChatSwitcher.css";

// A chat's id is never empty: the row for a new chat is told apart by it, and
// is the one checked while the open chat is not saved yet.
const NEW = "";

type Props = {
  /** The open chat's title; `null` for one not saved yet. */
  title: string | null;
  chats: ChatSummary[];
  activeChat: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
};

/**
 * The chat header's title as a menu of the folder's chats, with a new one on
 * top — the sidebar's list, for the IDE layout, which has no sidebar.
 * Archiving and deleting stay in the sidebar.
 */
export function ChatSwitcher({ title, chats, activeChat, onSelect, onNew }: Props) {
  return (
    <div className="chat-switcher">
      <Dropdown
        below
        label={title ?? "New chat"}
        title="Switch chats"
        heading="Chats in this folder"
        value={activeChat ?? NEW}
        onPick={(id) => (id === NEW ? onNew() : onSelect(id))}
        options={[
          { value: NEW, label: "New chat", hint: shortcutText("newChat") },
          ...chats.filter((chat) => !chat.archived).map((chat) => ({ value: chat.id, label: chat.title })),
        ]}
      />
    </div>
  );
}
