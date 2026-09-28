import { useEffect, useRef, useState } from "react";
import {
  Archive,
  ArchiveRestore,
  ChevronRight,
  Clock,
  CodeXml,
  GitBranch,
  Keyboard,
  MessageSquare,
  MessagesSquare,
  PanelLeft,
  Plus,
  Settings,
  Trash2,
  UserRound,
} from "lucide-react";
import { ChatMenu } from "./ChatMenu";
import { Dropdown } from "./Dropdown";
import { GettingStarted } from "./GettingStarted";
import { Modal } from "./Modal";
import { useShortcuts } from "../hooks/useShortcuts";
import { comboKeys, SHORTCUTS, type ShortcutId } from "../lib/shortcuts";
import type { ChatSummary } from "../lib/chat";
import type { AppMode, AsideTab } from "../types";
import "./Sidebar.css";

type Filter = "active" | "archived" | "all";

/** The list's one filter, as Claude Code's sidebar has it: archived chats are
    out of sight until asked for, not in a section of their own. */
const FILTERS: { value: Filter; label: string }[] = [
  { value: "active", label: "Active" },
  { value: "archived", label: "Archived" },
  { value: "all", label: "All" },
];

/** The registry's entries under their group headings, in its order. */
type Listed = (typeof SHORTCUTS)[ShortcutId] & { id: string };
const SHORTCUT_GROUPS = Object.entries(SHORTCUTS).reduce<Record<string, Listed[]>>((groups, [id, entry]) => {
  (groups[entry.group] ??= []).push({ id, ...entry });
  return groups;
}, {});

const MODES: { value: AppMode; label: string; title: string; icon: React.ReactNode }[] = [
  { value: "agent", label: "Agent", title: "Agent — works in the open folder with tools", icon: <CodeXml size={14} /> },
  { value: "chat", label: "Chat", title: "Chat — talks with the model, no files or tools", icon: <MessagesSquare size={14} /> },
];

const EMPTY: Record<Filter, string> = {
  active: "Every chat here is archived.",
  archived: "No archived chats.",
  all: "No chats yet.",
};

type Props = {
  mode: AppMode;
  onMode: (mode: AppMode) => void;
  chats: ChatSummary[];
  activeChat: string | null;
  onSelectChat: (id: string) => void;
  onNewChat: () => void;
  onArchiveChat: (id: string, archived: boolean) => void;
  onDeleteChat: (id: string) => void;
  onToggleCollapse: () => void;
  onOpenSettings: () => void;
  onOnboardingAction: (tab: AsideTab) => void;
};

export function Sidebar({
  mode,
  onMode,
  chats,
  activeChat,
  onSelectChat,
  onNewChat,
  onArchiveChat,
  onDeleteChat,
  onToggleCollapse,
  onOpenSettings,
  onOnboardingAction,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [filter, setFilter] = useState<Filter>("active");
  const [deleting, setDeleting] = useState<ChatSummary | null>(null);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  useShortcuts({ shortcuts: () => setShortcutsOpen((v) => !v) });
  const listed = chats.filter((chat) => filter === "all" || chat.archived === (filter === "archived"));
  const userWrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: PointerEvent) => {
      if (!userWrap.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setMenuOpen(false);
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [menuOpen]);

  return (
    <nav className="sidebar">
      <div className="sidebar-top">
        <button className="newchat" type="button" onClick={onNewChat}>
          <Plus size={14} />
          <span className="label">New chat</span>
        </button>
        <button className="iconbtn" type="button" title="Collapse panel" onClick={onToggleCollapse}>
          <PanelLeft size={15} />
        </button>
        <div className="mode-switch" role="tablist" aria-label="Mode">
          {MODES.map((m) => (
            <button
              key={m.value}
              type="button"
              role="tab"
              aria-selected={mode === m.value}
              className={`mode-opt${mode === m.value ? " active" : ""}`}
              title={m.title}
              onClick={() => onMode(m.value)}
            >
              {m.icon}
              <span>{m.label}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="group">
        {chats.length > 0 && (
          <div className="group-head">
            <span>Chats</span>
            <Dropdown
              label={FILTERS.find((f) => f.value === filter)?.label}
              title="Which chats to list"
              options={FILTERS}
              value={filter}
              onPick={(value) => setFilter(value as Filter)}
              below
              right
            />
          </div>
        )}
        {listed.length === 0 ? (
          <div className="empty">{chats.length === 0 ? "No chats yet." : EMPTY[filter]}</div>
        ) : (
          listed.map((chat) => (
            <div key={chat.id} className={`chat-row${chat.archived ? " archived" : ""}`}>
              <button
                type="button"
                className={`chat${chat.id === activeChat ? " active" : ""}`}
                onClick={() => onSelectChat(chat.id)}
                title={chat.archived ? "Archived" : chat.branchedFrom ? "A branch of an earlier chat" : undefined}
              >
                {chat.archived ? (
                  <Archive size={14} />
                ) : chat.branchedFrom ? (
                  <GitBranch size={14} />
                ) : (
                  <MessageSquare size={14} />
                )}
                <span>{chat.title}</span>
              </button>
              <ChatMenu
                items={[
                  chat.archived
                    ? {
                        id: "unarchive",
                        label: "Unarchive",
                        icon: <ArchiveRestore size={14} />,
                        onSelect: () => onArchiveChat(chat.id, false),
                      }
                    : {
                        id: "archive",
                        label: "Archive",
                        icon: <Archive size={14} />,
                        onSelect: () => onArchiveChat(chat.id, true),
                      },
                  {
                    id: "delete",
                    label: "Delete",
                    icon: <Trash2 size={14} />,
                    divided: true,
                    onSelect: () => setDeleting(chat),
                  },
                ]}
              />
            </div>
          ))
        )}
      </div>

      <Modal
        title="Delete this chat?"
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDeleting(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (deleting) onDeleteChat(deleting.id);
                setDeleting(null);
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p className="sidebar-delete-text">
          <b>{deleting?.title}</b> and its whole transcript will be gone for good. Archive it instead to keep it out of
          the list.
        </p>
      </Modal>

      <Modal title="Keyboard shortcuts" wide open={shortcutsOpen} onClose={() => setShortcutsOpen(false)}>
        <div className="sidebar-shortcut-groups">
          {Object.entries(SHORTCUT_GROUPS).map(([group, entries]) => (
            <section key={group} className="sidebar-shortcuts">
              <h3>{group}</h3>
              <dl>
                {entries.map(({ id, label, combos }) => (
                  <div key={id}>
                    <dt>{label}</dt>
                    <dd>
                      {combos.map((combo) => (
                        <span key={combo.code} className="combo">
                          {comboKeys(combo).map((k) => (
                            <kbd key={k}>{k}</kbd>
                          ))}
                        </span>
                      ))}
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </Modal>

      <div className="sidebar-bottom">
        <GettingStarted onAction={onOnboardingAction} onOpenSettings={onOpenSettings} />
        <div className="user-wrap" ref={userWrap}>
          <button
            type="button"
            className={`user${menuOpen ? " open" : ""}`}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((v) => !v)}
          >
            <UserRound size={16} />
            <span className="meta">Account</span>
            <ChevronRight className="chev" size={12} />
          </button>
          {menuOpen && (
            <div className="user-menu" role="menu">
              <button
                className="user-menu-item"
                role="menuitem"
                type="button"
                onClick={() => {
                  setMenuOpen(false);
                  onOpenSettings();
                }}
              >
                <span className="ico">
                  <Settings size={14} />
                </span>
                Settings
              </button>
              <button
                className="user-menu-item"
                role="menuitem"
                type="button"
                onClick={() => {
                  setMenuOpen(false);
                  setShortcutsOpen(true);
                }}
              >
                <span className="ico">
                  <Keyboard size={14} />
                </span>
                Keyboard shortcuts
              </button>
              <button
                className="user-menu-item"
                role="menuitem"
                type="button"
                onClick={() => setMenuOpen(false)}
              >
                <span className="ico">
                  <Clock size={14} />
                </span>
                About Kibo Agent
              </button>
            </div>
          )}
        </div>
      </div>
    </nav>
  );
}
