import { useEffect, useRef, useState } from "react";
import { appVersion, openExternal } from "../lib/window";
import {
  AppWindow,
  Archive,
  ArchiveRestore,
  ChevronRight,
  CircleCheck,
  CodeXml,
  FolderInput,
  GitBranch,
  Info,
  Keyboard,
  Loader2,
  MessageCircleQuestion,
  MessageSquare,
  MessagesSquare,
  Moon,
  Pencil,
  Pin,
  PinOff,
  Search,
  Settings,
  ShipWheel,
  Sun,
  Trash2,
} from "lucide-react";
import { ChangingText } from "./ChangingText";
import { ChatMenu } from "./ChatMenu";
import { Dropdown } from "./Dropdown";
import { GettingStarted } from "./GettingStarted";
import { type PAGES, type PageId } from "./pages";
import { Modal } from "./Modal";
import { useShortcuts } from "../hooks/useShortcuts";
import { useStoredState } from "../hooks/useStoredState";
import { comboKeys, matches, SHORTCUTS, type ShortcutId } from "../lib/shortcuts";
import type { ChatGroup, ChatSummary } from "../lib/savedChats";
import { APP_MODES, type AppMode, type AsideTab, type ChatActivity } from "../types";
import type { Scheme } from "../hooks/useTheme";
import "./Sidebar.css";

type Filter = "active" | "archived" | "all";

/** Releases, and the `nightly` pre-release built from every push to main. */
const RELEASES = "https://github.com/gribanoveu/kibo-agent/releases";

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

// Keyed by mode, so a mode added to APP_MODES does not build until it has a row here.
const MODES: Record<AppMode, { label: string; title: string; icon: React.ReactNode }> = {
  agent: { label: "Agent", title: "Agent — works in the open folder with tools", icon: <CodeXml size={14} /> },
  chat: { label: "Chat", title: "Chat — talks with the model, no files or tools", icon: <MessagesSquare size={14} /> },
};

const EMPTY: Record<Filter, string> = {
  active: "Every chat here is archived.",
  archived: "No archived chats.",
  all: "No chats yet.",
};

/** The list under its headings: what was touched today, and the rest, each in the order it came. */
function byDay(chats: ChatSummary[]) {
  const today = new Date().toDateString();
  const isToday = (chat: ChatSummary) => new Date(chat.updatedAt).toDateString() === today;
  return [
    { day: "Today", chats: chats.filter(isToday) },
    { day: "Earlier", chats: chats.filter((chat) => !isToday(chat)) },
  ].filter((group) => group.chats.length > 0);
}

/** `useStoredState`'s screen for the collapsed-folder list. */
const isStringArray = (value: unknown): value is string[] => Array.isArray(value) && value.every((item) => typeof item === "string");

type Props = {
  mode: AppMode;
  onMode: (mode: AppMode) => void;
  /** The pages to list under the modes — those of features that are off left out (`pagesShown`). */
  pages: readonly (typeof PAGES)[number][];
  /** The page open in place of the chat (pages.tsx); no mode is lit then. */
  page: PageId | null;
  onPage: (page: PageId) => void;
  chats: ChatSummary[];
  /** The folders of the current list — the open folder's, or Chat mode's shared set. */
  groups: ChatGroup[];
  /** What the collapsed-folder memory is scoped by: the open folder's path, null for Chat mode. */
  groupScope: string | null;
  activeChat: string | null;
  chatActivity?: Record<string, ChatActivity>;
  onSelectChat: (id: string) => void;
  onArchiveChat: (id: string, archived: boolean) => void;
  onPinChat: (id: string, pinned: boolean) => void;
  onMoveChat: (id: string, groupId: string | null) => void;
  /** Makes a folder; resolves to its id, or the refusal as `error` (the dialog stays open). */
  onCreateGroup: (name: string) => Promise<{ id?: string; error?: string }>;
  /** Renames a folder; a refusal comes back as `error` and keeps the dialog open. */
  onRenameGroup: (id: string, name: string) => Promise<{ error?: string }>;
  onDeleteGroup: (id: string) => void;
  onDeleteChat: (id: string) => void;
  onOpenSettings: () => void;
  onOnboardingAction: (tab: AsideTab) => void;
  /** The side the window shows now, and a switch to the other one. */
  themeSide: Scheme;
  onToggleTheme: () => void;
};

export function Sidebar({
  mode,
  onMode,
  pages,
  page,
  onPage,
  chats,
  groups,
  groupScope,
  activeChat,
  chatActivity = {},
  onSelectChat,
  onArchiveChat,
  onPinChat,
  onMoveChat,
  onCreateGroup,
  onRenameGroup,
  onDeleteGroup,
  onDeleteChat,
  onOpenSettings,
  onOnboardingAction,
  themeSide,
  onToggleTheme,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [filter, setFilter] = useState<Filter>("active");
  const [query, setQuery] = useState("");
  const [deleting, setDeleting] = useState<ChatSummary | null>(null);
  const [moving, setMoving] = useState<ChatSummary | null>(null);
  /** A folder being named: `group` set is a rename, unset is a new one. */
  const [naming, setNaming] = useState<{ group: ChatGroup | null; name: string } | null>(null);
  const [nameError, setNameError] = useState<string | null>(null);
  const [deletingGroup, setDeletingGroup] = useState<ChatGroup | null>(null);
  // A folder shut by id, remembered per list: collapse is about the view,
  // not the chats, so it lives in window storage and not in the store.
  const [collapsed, setCollapsed] = useStoredState<string[]>(`kibo-chat-groups-${groupScope ?? ""}`, [], isStringArray);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  useShortcuts({ shortcuts: () => setShortcutsOpen((v) => !v) });
  const [aboutOpen, setAboutOpen] = useState(false);
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    if (aboutOpen && version === null) void appVersion().then(setVersion, () => setVersion("unknown"));
  }, [aboutOpen, version]);
  const wanted = query.trim().toLowerCase();
  const listed = chats.filter(
    (chat) => (filter === "all" || chat.archived === (filter === "archived")) && chat.title.toLowerCase().includes(wanted),
  );
  // Pinned first, then the folders, then the days — and a row shown twice
  // reads as two chats, so each of the three takes a chat out of the rest.
  // Folders are the working view: only the active filter draws them. A chat
  // whose folder is not among the known ones (a folder deleted mid-move, a
  // list fresher than the folders) is never lost: it falls into the days.
  const groupIds = new Set(groups.map((group) => group.id));
  const pinnedRows = filter === "active" ? listed.filter((chat) => chat.pinned) : [];
  const folderRows = filter === "active" ? listed.filter((chat) => !chat.pinned && chat.group != null && groupIds.has(chat.group)) : [];
  const dayRows =
    filter === "active" ? listed.filter((chat) => !chat.pinned && (chat.group == null || !groupIds.has(chat.group))) : listed;
  const userWrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const onDown = (e: PointerEvent) => {
      if (!userWrap.current?.contains(e.target as Node)) setMenuOpen(false);
    };
    const onKey = (e: KeyboardEvent) => matches(e, "close") && setMenuOpen(false);
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [menuOpen]);

  const chatRow = (chat: ChatSummary) => (
    <div key={chat.id} className={`chat-row${chat.archived ? " archived" : ""}`}>
      <button
        type="button"
        className={`chat${page === null && chat.id === activeChat ? " active" : ""}`}
        onClick={() => onSelectChat(chat.id)}
        title={chat.archived ? "Archived" : chat.branchedFrom ? "A branch of an earlier chat" : undefined}
      >
        {chatActivity[chat.id] === "running" ? (
          <Loader2 className="chat-activity chat-activity-running" size={14} aria-label="Turn running" />
        ) : chatActivity[chat.id] === "awaitingApproval" ? (
          <MessageCircleQuestion className="chat-activity chat-activity-awaiting" size={14} aria-label="Approval needed" />
        ) : chatActivity[chat.id] === "finished" ? (
          <CircleCheck className="chat-activity" size={14} aria-label="Finished in background" />
        ) : chat.archived ? (
          <Archive size={14} />
        ) : chat.branchedFrom ? (
          <GitBranch size={14} />
        ) : chat.role === "kubernetes" ? (
          <ShipWheel size={14} />
        ) : chat.role === "prototype" ? (
          <AppWindow size={14} />
        ) : chat.role === "assistant" ? (
          <MessagesSquare size={14} />
        ) : (
          <MessageSquare size={14} />
        )}
        <ChangingText text={chat.title} />
        {chat.pinned && <Pin size={12} className="chat-pin" aria-label="Pinned" />}
      </button>
      <ChatMenu
        items={[
          {
            id: "move",
            label: "Move to folder…",
            icon: <FolderInput size={14} />,
            onSelect: () => setMoving(chat),
          },
          // Pin is the working view's action; on an archived row it would
          // change a state no list shows until the chat is back.
          ...(!chat.archived
            ? [
                chat.pinned
                  ? {
                      id: "unpin",
                      label: "Unpin",
                      icon: <PinOff size={14} />,
                      onSelect: () => onPinChat(chat.id, false),
                    }
                  : {
                      id: "pin",
                      label: "Pin",
                      icon: <Pin size={14} />,
                      onSelect: () => onPinChat(chat.id, true),
                    },
              ]
            : []),
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
  );

  const toggleFolder = (id: string) =>
    setCollapsed((shut) => (shut.includes(id) ? shut.filter((one) => one !== id) : [...shut, id]));

  const saveName = async () => {
    if (!naming?.name.trim()) return;
    let error: string | undefined;
    let id: string | undefined;
    if (naming.group) {
      ({ error } = await onRenameGroup(naming.group.id, naming.name));
    } else {
      ({ error, id } = await onCreateGroup(naming.name));
    }
    if (error) {
      // The refusal names the fix; the dialog stays open with it.
      setNameError(error);
      return;
    }
    setNaming(null);
    // A folder made from the move dialog is made for the chat being moved.
    if (!naming.group && moving) {
      onMoveChat(moving.id, id ?? null);
      setMoving(null);
    }
  };

  return (
    <nav className="sidebar">
      <div className="sidebar-top">
        <div className="mode-switch" role="tablist" aria-label="Mode">
          {APP_MODES.map((value) => {
            const active = mode === value;
            return (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={active}
                className={`mode-opt${active ? " active" : ""}`}
                title={MODES[value].title}
                onClick={() => onMode(value)}
              >
                <span className="mode-tile">{MODES[value].icon}</span>
                <span className="mode-label">{MODES[value].label}</span>
              </button>
            );
          })}
        </div>
        <label className="sidebar-search">
          <Search size={14} />
          <input
            type="text"
            placeholder="Search chats"
            aria-label="Search chats"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (matches(e, "close")) setQuery("");
            }}
          />
        </label>
        {pages.length > 0 && <div className="sidebar-pages-head">Workspace</div>}
        <div className="mode-switch sidebar-pages" role="tablist" aria-label="Pages">
          {pages.map(({ id, label, title, icon: Icon }) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={page === id}
              className={`mode-opt${page === id ? " active" : ""}`}
              title={title}
              onClick={() => onPage(id)}
            >
              <Icon size={15} />
              <span className="mode-label">{label}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="group">
        {(chats.length > 0 || groups.length > 0) && (
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
        {/* With folders alive below, their Empty lines are the state — a
            "No chats yet." above them contradicts what is on screen. */}
        {listed.length === 0 && !(chats.length === 0 && groups.length > 0) && (
          <div className="empty">{chats.length === 0 ? "No chats yet." : wanted ? "No chats match." : EMPTY[filter]}</div>
        )}
        {pinnedRows.length > 0 && (
          <section>
            <div className="chat-day">Pinned</div>
            {pinnedRows.map(chatRow)}
          </section>
        )}
        {filter === "active" &&
          groups.map((group) => {
            const rows = folderRows.filter((chat) => chat.group === group.id);
            // Searching, a folder with nothing matching is noise — the days
            // disappear whole the same way; "Empty" is for a folder at rest.
            if (wanted && rows.length === 0) return null;
            const shut = collapsed.includes(group.id);
            return (
              <section key={group.id}>
                <div className="chat-day chat-folder-head">
                  <button
                    type="button"
                    className="chat-folder-toggle"
                    aria-expanded={!shut}
                    onClick={() => toggleFolder(group.id)}
                  >
                    <ChevronRight size={12} className={shut ? undefined : "chat-folder-open"} aria-hidden />
                    <span className="chat-folder-name">{group.name}</span>
                    <span className="chat-folder-count">{rows.length}</span>
                  </button>
                  <ChatMenu
                    items={[
                      {
                        id: "rename",
                        label: "Rename…",
                        icon: <Pencil size={14} />,
                        onSelect: () => {
                          setNaming({ group, name: group.name });
                          setNameError(null);
                        },
                      },
                      {
                        id: "delete",
                        label: "Delete…",
                        icon: <Trash2 size={14} />,
                        divided: true,
                        onSelect: () => setDeletingGroup(group),
                      },
                    ]}
                  />
                </div>
                {!shut && (rows.length > 0 ? rows.map(chatRow) : !wanted && <div className="chat-folder-empty">Empty</div>)}
              </section>
            );
          })}
        {listed.length > 0 &&
          byDay(dayRows).map(({ day, chats: rows }) => (
            <section key={day}>
              <div className="chat-day">{day}</div>
              {rows.map(chatRow)}
            </section>
          ))}
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

      <Modal title={`Move “${moving?.title ?? ""}” to`} open={moving !== null} onClose={() => setMoving(null)}>
        <Dropdown
          label="Pick a folder"
          title="Which folder holds the chat"
          heading="Move to"
          options={[
            { value: "", label: "No folder" },
            ...groups.map((group) => ({ value: group.id, label: group.name })),
            { value: "__new__", label: "New folder…" },
          ]}
          value=""
          below
          onPick={(value) => {
            if (value === "__new__") {
              // The move dialog stays open beneath the name dialog: the
              // folder being made is made for this chat.
              setNaming({ group: null, name: "" });
              setNameError(null);
              return;
            }
            if (moving) onMoveChat(moving.id, value === "" ? null : value);
            setMoving(null);
          }}
        />
      </Modal>

      <Modal
        title={naming?.group ? `Rename “${naming.group.name}”` : "New folder"}
        open={naming !== null}
        onClose={() => setNaming(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setNaming(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              disabled={!naming?.name.trim()}
              onClick={() => void saveName()}
            >
              {naming?.group ? "Save" : "Create"}
            </button>
          </>
        }
      >
        <label className="sidebar-field">
          <span>Name</span>
          <input
            type="text"
            value={naming?.name ?? ""}
            onChange={(e) => {
              if (naming) setNaming({ ...naming, name: e.target.value });
              setNameError(null);
            }}
            onKeyDown={(e) => {
              if (!matches(e, "accept")) return;
              e.preventDefault();
              void saveName();
            }}
          />
        </label>
        {nameError && <p className="sidebar-field-error">{nameError}</p>}
      </Modal>

      <Modal
        title={`Delete folder “${deletingGroup?.name ?? ""}”?`}
        open={deletingGroup !== null}
        onClose={() => setDeletingGroup(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDeletingGroup(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (deletingGroup) onDeleteGroup(deletingGroup.id);
                setDeletingGroup(null);
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p className="sidebar-delete-text">
          Its chats stay in the list, filed under no folder.
        </p>
      </Modal>

      <Modal
        title="About Kibo Agent"
        open={aboutOpen}
        onClose={() => setAboutOpen(false)}
        footer={
          <button type="button" className="btn btn-primary" title={RELEASES} onClick={() => void openExternal(RELEASES)}>
            Download page
          </button>
        }
      >
        <p className="sidebar-about-version">Version {version ?? "…"}</p>
        <p className="sidebar-about-note">
          New versions are on GitHub: releases, and a nightly build of main marked as a pre-release.
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
            <span className="user-avatar" aria-hidden="true">
              A
            </span>
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
                  onToggleTheme();
                }}
              >
                <span className="ico">{themeSide === "light" ? <Moon size={14} /> : <Sun size={14} />}</span>
                {themeSide === "light" ? "Dark theme" : "Light theme"}
              </button>
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
                onClick={() => {
                  setMenuOpen(false);
                  setAboutOpen(true);
                }}
              >
                <span className="ico">
                  <Info size={14} />
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
