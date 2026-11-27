import { useEffect, useRef, useState } from "react";
import { appVersion, openExternal } from "../lib/window";
import {
  AppWindow,
  ArrowDown,
  ArrowUp,
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
  Plus,
  Search,
  X,
  Settings,
  ShipWheel,
  Sun,
  Trash2,
} from "lucide-react";
import { ChangingText } from "./ChangingText";
import { ChatMenu } from "./ChatMenu";
import { Dropdown } from "./Dropdown";
import { type PAGES, type PageId } from "./pages";
import { Modal } from "./Modal";
import { useShortcuts } from "../hooks/useShortcuts";
import { isBoolean, useStoredState } from "../hooks/useStoredState";
import { comboKeys, matches, SHORTCUTS, type ShortcutId } from "../lib/shortcuts";
import type { ChatGroup, ChatSummary } from "../lib/savedChats";
import { APP_MODES, type AppMode, type ChatActivity } from "../types";
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

/** One section of the list: an open folder's chats, or Chat mode's (`path` null). */
export type ChatSection = {
  path: string | null;
  chats: ChatSummary[];
  /** The main checkout, when the folder is a linked worktree of it. */
  worktreeOf?: string | null;
};

/** The last part of a folder's path, as its section head names it. */
const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** Whether the filter and the search leave a chat in the list. */
const shownBy = (chat: ChatSummary, filter: Filter, wanted: string) =>
  (filter === "all" || chat.archived === (filter === "archived")) && chat.title.toLowerCase().includes(wanted);

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
  /** The list, a section per open folder in Agent mode (F-8.6); Chat mode's one set has `path` null. */
  sections: ChatSection[];
  /** The folder the chat on screen is in — its section's head is lit. */
  activeFolder?: string | null;
  activeChat: string | null;
  chatActivity?: Record<string, ChatActivity>;
  /** `folder` is the section the row is in: opening its chat makes it the active folder. */
  onSelectChat: (id: string, folder: string | null) => void;
  onArchiveChat: (id: string, archived: boolean) => void;
  onPinChat: (id: string, pinned: boolean) => void;
  onMoveChat: (id: string, groupId: string | null) => void;
  /** The chat folders of the list on screen: Chat mode's, or the ones every project shares. */
  groups: ChatGroup[];
  /** Makes a chat folder; resolves to its id, or the refusal as `error` (the dialog stays open). */
  onCreateGroup: (name: string) => Promise<{ id?: string; error?: string }>;
  /** Renames a chat folder; a refusal comes back as `error` and keeps the dialog open. */
  onRenameGroup: (id: string, name: string) => Promise<{ error?: string }>;
  onDeleteGroup: (id: string) => void;
  /** Agent mode's section heads: a new chat in that folder, and closing it. */
  onNewChatIn?: (folder: string) => void;
  onCloseFolder?: (folder: string) => void;
  /** Moves a folder's section one place up (-1) or down (+1) (FO-1). */
  onMoveFolder?: (folder: string, dir: -1 | 1) => void;
  onDeleteChat: (id: string) => void;
  onOpenSettings: () => void;
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
  sections,
  activeFolder = null,
  activeChat,
  chatActivity = {},
  onSelectChat,
  onArchiveChat,
  onPinChat,
  onMoveChat,
  groups,
  onCreateGroup,
  onRenameGroup,
  onDeleteGroup,
  onNewChatIn,
  onCloseFolder,
  onMoveFolder,
  onDeleteChat,
  onOpenSettings,
  themeSide,
  onToggleTheme,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [filter, setFilter] = useState<Filter>("active");
  const [query, setQuery] = useState("");
  const [deleting, setDeleting] = useState<ChatSummary | null>(null);
  /** A folder being named: `group` set is a rename, unset a new one; a new
      one made from a chat's Move to folder carries `chat` into it. */
  const [naming, setNaming] = useState<{ group: ChatGroup | null; name: string; chat: ChatSummary | null } | null>(
    null,
  );
  const [nameError, setNameError] = useState<string | null>(null);
  const [deletingGroup, setDeletingGroup] = useState<ChatGroup | null>(null);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  useShortcuts({ shortcuts: () => setShortcutsOpen((v) => !v) });
  const [aboutOpen, setAboutOpen] = useState(false);
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    if (aboutOpen && version === null) void appVersion().then(setVersion, () => setVersion("unknown"));
  }, [aboutOpen, version]);
  const wanted = query.trim().toLowerCase();
  const anything = groups.length > 0 || sections.some((section) => section.chats.length > 0);
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

  const chatRow = (chat: ChatSummary, { path: scope }: ChatSection) => (
    <div key={chat.id} className={`chat-row${chat.archived ? " archived" : ""}`}>
      <button
        type="button"
        className={`chat${page === null && chat.id === activeChat ? " active" : ""}`}
        onClick={() => onSelectChat(chat.id, scope)}
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
            label: "To folder",
            icon: <FolderInput size={14} />,
            onSelect: () => {},
            // The folders are the menu's second level — picking one is the
            // move, and a folder is never made without the chat going in.
            submenu: [
              { id: "no-folder", label: "No folder", onSelect: () => onMoveChat(chat.id, null) },
              ...groups.map((group) => ({ id: group.id, label: group.name, onSelect: () => onMoveChat(chat.id, group.id) })),
              {
                id: "new-folder",
                label: "New folder…",
                divided: true,
                onSelect: () => {
                  setNaming({ group: null, name: "", chat });
                  setNameError(null);
                },
              },
            ],
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
    // A folder made from a chat's Move to folder is made for that chat.
    if (!naming.group && naming.chat) {
      onMoveChat(naming.chat.id, id ?? null);
    }
  };

  return (
    <nav className="sidebar">
      <div className="sidebar-top">
        <div className="mode-switch" role="tablist" aria-label="Mode" data-tour="mode">
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
              // The tour rings this row before it opens what is behind it.
              data-tour={id === "catalog" ? "catalog-tab" : undefined}
              onClick={() => onPage(id)}
            >
              <Icon size={15} />
              <span className="mode-label">{label}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="group">
        {anything && (
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
        {/* Chat folders on screen are the state; "No chats yet." above them contradicts it. */}
        {sections.length === 0 && groups.length === 0 && <div className="empty">No chats yet.</div>}
        {/* An open folder with nothing matching draws no section; with none matching anywhere, the list says so. */}
        {wanted &&
          sections.every((section) => section.path !== null) &&
          !sections.some((section) => section.chats.some((chat) => shownBy(chat, filter, wanted))) && <div className="empty">No chats match.</div>}
        {/* The chat folders the user made, every folder's, above the folders'
            sections — kept on top as pinned chats are within theirs. */}
        {filter === "active" && (
          <ChatFolders
            // Keyed by mode: the folded set is read from storage when it mounts,
            // and Chat mode's is not Agent mode's.
            key={mode}
            mode={mode}
            groups={groups}
            sections={sections}
            wanted={wanted}
            row={chatRow}
            onRenameGroup={(group) => {
              setNaming({ group, name: group.name, chat: null });
              setNameError(null);
            }}
            onDeleteGroup={setDeletingGroup}
          />
        )}
        {sections.map((section, i) => (
          <SectionList
            key={section.path ?? ""}
            section={section}
            groups={groups}
            filter={filter}
            wanted={wanted}
            active={section.path !== null && section.path === activeFolder}
            row={(chat) => chatRow(chat, section)}
            onNewChat={section.path !== null && onNewChatIn ? () => onNewChatIn(section.path as string) : undefined}
            onClose={section.path !== null && onCloseFolder ? () => onCloseFolder(section.path as string) : undefined}
            onMoveUp={section.path !== null && onMoveFolder && i > 0 ? () => onMoveFolder(section.path as string, -1) : undefined}
            onMoveDown={section.path !== null && onMoveFolder && i < sections.length - 1 ? () => onMoveFolder(section.path as string, 1) : undefined}
          />
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

type SectionProps = {
  section: ChatSection;
  /** The chat folders: a chat in one of them is drawn there, not here. */
  groups: ChatGroup[];
  filter: Filter;
  wanted: string;
  /** The active folder's section: its head is lit. */
  active: boolean;
  row: (chat: ChatSummary) => React.ReactNode;
  /** Set for an open folder's section — the head that names it. */
  onNewChat?: () => void;
  onClose?: () => void;
  /** Moving the section among its neighbours (FO-1); unset at an edge. */
  onMoveUp?: () => void;
  onMoveDown?: () => void;
};

/** A section's rows the filter and search leave, split three ways: pinned,
 * in one of the chat folders, and the rest — a row shown twice reads as two
 * chats, so each takes a chat out of the rest. Chat folders are the working
 * view: only the active filter splits. A chat whose folder is not among the
 * known ones (a folder deleted mid-move, a list fresher than the folders) is
 * never lost: it falls into the rest. */
function rowsOf({ chats }: ChatSection, groups: ChatGroup[], filter: Filter, wanted: string) {
  const listed = chats.filter((chat) => shownBy(chat, filter, wanted));
  if (filter !== "active") return { listed, pinned: [], foldered: [], rest: listed };
  const groupIds = new Set(groups.map((group) => group.id));
  const foldered = (chat: ChatSummary) => !chat.pinned && chat.group != null && groupIds.has(chat.group);
  return {
    listed,
    pinned: listed.filter((chat) => chat.pinned),
    foldered: listed.filter(foldered),
    rest: listed.filter((chat) => !chat.pinned && !foldered(chat)),
  };
}

type FoldersProps = {
  /** Whose folders: Chat mode's and the projects' fold apart in window storage. */
  mode: AppMode;
  groups: ChatGroup[];
  sections: ChatSection[];
  wanted: string;
  row: (chat: ChatSummary, section: ChatSection) => React.ReactNode;
  onRenameGroup: (group: ChatGroup) => void;
  onDeleteGroup: (group: ChatGroup) => void;
};

/** The chat folders, drawn above every section: each holds its chats from
 * every open folder, in the sections' order. Only the active filter draws
 * them. */
function ChatFolders({ mode, groups, sections, wanted, row, onRenameGroup, onDeleteGroup }: FoldersProps) {
  // A chat folder shut by id, remembered per list: collapse is about the
  // view, not the chats, so it lives in window storage and not in the store.
  const [collapsed, setCollapsed] = useStoredState<string[]>(`kibo-chat-groups-${mode}`, [], isStringArray);
  const toggleFolder = (id: string) =>
    setCollapsed((shut) => (shut.includes(id) ? shut.filter((one) => one !== id) : [...shut, id]));
  const foldered = sections.flatMap((section) =>
    rowsOf(section, groups, "active", wanted).foldered.map((chat) => ({ chat, section })),
  );
  return groups.map((group) => {
    const rows = foldered.filter(({ chat }) => chat.group === group.id);
    // Searching, a folder with nothing matching is noise — the rest
    // disappear whole the same way; "Empty" is for a folder at rest.
    if (wanted && rows.length === 0) return null;
    const shut = collapsed.includes(group.id);
    return (
      <section key={group.id} className="chat-folder">
        {/* The name is cut to one line; the whole of it on hover. */}
        <div className="chat-day chat-folder-head" title={group.name}>
          <button type="button" className="chat-folder-toggle" aria-expanded={!shut} onClick={() => toggleFolder(group.id)}>
            <span className="chat-folder-name">{group.name}</span>
            <span className="chat-folder-count">{rows.length}</span>
            <ChevronRight size={12} className={shut ? "chat-folder-chev" : "chat-folder-chev chat-folder-open"} aria-hidden />
          </button>
          <ChatMenu
            items={[
              { id: "rename", label: "Rename…", icon: <Pencil size={14} />, onSelect: () => onRenameGroup(group) },
              { id: "delete", label: "Delete…", icon: <Trash2 size={14} />, divided: true, onSelect: () => onDeleteGroup(group) },
            ]}
          />
        </div>
        {!shut &&
          (rows.length > 0 ? rows.map(({ chat, section }) => row(chat, section)) : !wanted && <div className="chat-folder-empty">Empty</div>)}
      </section>
    );
  });
}

/** One section's rows under its head: pinned first, then the rest — its chat
 * folders stand above every section (`ChatFolders`). */
function SectionList({ section, groups, filter, wanted, active, row, onNewChat, onClose, onMoveUp, onMoveDown }: SectionProps) {
  const { path, chats, worktreeOf } = section;
  // An open folder's whole section folds under its head, remembered per
  // folder; a search shows what matches in it all the same.
  const [folded, setFolded] = useStoredState(`kibo-folder-folded-${path ?? ""}`, false, isBoolean);
  const [showAll, setShowAll] = useState(false);
  const open = path === null || !folded || !!wanted;
  const { listed, pinned: pinnedRows, rest: restRows } = rowsOf(section, groups, filter, wanted);
  // A folder's long list shows its first four chats; the rest wait behind a
  // button. A search shows every match — hiding one it found is noise.
  const shownRest = path !== null && !wanted && !showAll ? restRows.slice(0, 4) : restRows;
  // Searching, a folder with nothing matching says nothing — its head stays
  // out of the way the rest does.
  if (path !== null && wanted && listed.length === 0) return null;

  return (
    <div className="chat-section">
      {path !== null && (
        <div
          className={`chat-workspace-head${active ? " active" : ""}`}
          title={worktreeOf ? `A worktree of ${worktreeOf}\nThis folder: ${path}` : path}
        >
          <button type="button" className="chat-workspace-toggle" aria-expanded={open} onClick={() => setFolded(!folded)}>
            <ChevronRight size={12} className={open ? "chat-workspace-chev chat-workspace-open" : "chat-workspace-chev"} aria-hidden />
            <span className="chat-workspace-name">
              {folderName(path)}
              {worktreeOf && <span className="chat-workspace-of"> · worktree of {folderName(worktreeOf)}</span>}
            </span>
          </button>
          {onNewChat && (
            <button type="button" className="chat-workspace-new" title={`New chat in ${folderName(path)}`} aria-label={`New chat in ${folderName(path)}`} onClick={onNewChat}>
              <Plus size={14} />
            </button>
          )}
          {onClose && (
            <ChatMenu
              items={[
                ...(onNewChat ? [{ id: "new", label: "New chat", icon: <Plus size={14} />, onSelect: onNewChat }] : []),
                // Shown while either neighbour exists; the edge one is disabled.
                ...(onMoveUp || onMoveDown
                  ? [
                      { id: "up", label: "Move up", icon: <ArrowUp size={14} />, disabled: !onMoveUp, hint: onMoveUp ? undefined : "Already first", onSelect: () => onMoveUp?.() },
                      { id: "down", label: "Move down", icon: <ArrowDown size={14} />, disabled: !onMoveDown, hint: onMoveDown ? undefined : "Already last", onSelect: () => onMoveDown?.() },
                    ]
                  : []),
                { id: "close", label: "Close folder", icon: <X size={14} />, divided: true, onSelect: onClose },
              ]}
            />
          )}
        </div>
      )}
      {open && (
        <>
          {/* With chat folders alive above, their Empty lines are the state —
              a "No chats yet." under them contradicts what is on screen. */}
          {listed.length === 0 && !(chats.length === 0 && groups.length > 0) && (
            <div className="empty">{chats.length === 0 ? "No chats yet." : wanted ? "No chats match." : EMPTY[filter]}</div>
          )}
          {pinnedRows.length > 0 && (
            <section>
              <div className="chat-day">Pinned</div>
              {pinnedRows.map(row)}
            </section>
          )}
          {/* The rest under no heading, in the order the store gives — the
              last touched first. */}
          {restRows.length > 0 && (
            <section className="chat-rest">
              {shownRest.map(row)}
              {shownRest.length < restRows.length && (
                <button type="button" className="chat-show-all" onClick={() => setShowAll(true)}>
                  Show all {restRows.length} chats
                </button>
              )}
            </section>
          )}
        </>
      )}
    </div>
  );
}
