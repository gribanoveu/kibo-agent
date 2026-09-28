import { useCallback, useEffect, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatPanel } from "./components/ChatPanel";
import { Composer } from "./components/Composer";
import { PlainChat } from "./components/PlainChat";
import { FolderTab } from "./components/FolderTab";
import { AsidePanel } from "./components/AsidePanel";
import { FileViewer } from "./components/FileViewer";
import { PANES, type Dock, type PaneContext } from "./components/panes";
import { Modal } from "./components/Modal";
import { Settings } from "./components/Settings";
import { ToolLog } from "./components/ToolLog";
import { ConfigFileEditor } from "./components/ConfigFileEditor";
import { PanelResizeHandle } from "./components/PanelResizeHandle";
import { Toast } from "./components/Toast";
import { WindowControls } from "./components/WindowControls";
import { useAgentTurn } from "./hooks/useAgentTurn";
import { usePlainChat } from "./hooks/usePlainChat";
import { useChatHistory } from "./hooks/useChatHistory";
import { useMediaQuery } from "./hooks/useMediaQuery";
import { useLlmSettings } from "./hooks/useLlmSettings";
import { useWorkspace } from "./hooks/useWorkspace";
import { useIndexStatus } from "./hooks/useIndexStatus";
import { useMcp } from "./hooks/useMcp";
import { useHooks } from "./hooks/useHooks";
import { useSkills } from "./hooks/useSkills";
import { useToolLog } from "./hooks/useToolLog";
import { roomFor, usePanelSizes } from "./hooks/usePanelSizes";
import { useTheme } from "./hooks/useTheme";
import { useChatFontSize } from "./hooks/useChatFontSize";
import { useGitBranch, useWorktreeOf } from "./hooks/useGitBranch";
import { useChangeTotals } from "./hooks/useChangeTotals";
import { useToast } from "./hooks/useToast";
import { nativeFrame, startWindowDrag, toggleMaximizeWindow } from "./lib/window";
import { pickSavePath } from "./lib/dialog";
import { useBackendSetting } from "./hooks/useBackendSetting";
import { useApprovalMemory } from "./hooks/useApprovalMemory";
import { useFolderConversation } from "./hooks/useFolderConversation";
import { useTurnAlerts } from "./hooks/useTurnAlerts";
import { useRunningProcesses } from "./hooks/useProcesses";
import { playSound } from "./lib/sounds";
import { DEFAULT_ALERTS, isAlertPrefs } from "./lib/attention";
import { isBoolean, useStoredState } from "./hooks/useStoredState";
import { McpServerForm, HookForm } from "./components/ConfigEntryForm";
import { removeHook, removeMcpServer } from "./lib/configEntries";
import { mergeHooks, mergeMcp } from "./lib/configSnippets";
import { changesShown, openPane, toggleChanges, togglePane, toggleTerminal, type Docks } from "./lib/docks";
import { useShortcuts } from "./hooks/useShortcuts";
import { exportChat, setConversationMode, withLanguageReminder, type ConversationMode } from "./lib/chat";
import { isAppMode, isAsideTab, type AppMode, type AsideTab } from "./types";
import { useFolderSwitch } from "./hooks/useFolderSwitch";
import { fileLinkPath, useOpenFiles } from "./hooks/useOpenFiles";
import { FolderSwitchDialog } from "./components/FolderSwitchDialog";
import { useBranchPicker } from "./hooks/useBranchPicker";
import { BranchConflictDialog } from "./components/BranchConflictDialog";
import { useWorktreeRemoval } from "./hooks/useWorktreeRemoval";
import { WorktreeRemoveDialog } from "./components/WorktreeRemoveDialog";
import { RewindDialog } from "./components/RewindDialog";
import { useRewind } from "./hooks/useRewind";
import { expandTemplate, fileCommands, typedCommand, type SlashCommand } from "./lib/slashCommands";
import initPrompt from "./prompts/init.md?raw";
import { useCommandFiles } from "./hooks/useCommandFiles";
import "./App.css";
import type { AgentFocus } from "./lib/describeTool";

// Titlebar drag: single press drags the window, double press zooms it — the macOS
// titlebar contract, driven explicitly so clicks on the controls stay clicks.
const dragOrMaximize = (e: React.MouseEvent) => {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
  // Without this the webview keeps extending a text selection while the window
  // moves under the cursor, which flickers through whatever it passes over.
  e.preventDefault();
  if (e.detail === 2) toggleMaximizeWindow();
  else startWindowDrag();
};

/** A stored pane id, if it still opens in `dock` — a pane may have moved docks since it was stored. */
const isPaneIn =
  (dock: Dock) =>
  (value: unknown): value is AsideTab =>
    isAsideTab(value) && PANES.find((p) => p.id === value)?.dock === dock;
// Any pane may sit under the top one: Changes goes there when the top is taken.
const isBottomTab = (value: unknown): value is AsideTab | null => value === null || isAsideTab(value);

/** A config file's dialog: the whole file, one entry of it (null for a new one), or closed. */
type EntryDialog<K> = "json" | { entry: K | null } | null;

/** What "Implement in Agent mode" says on the user's behalf. Shown in the transcript like anything they type. */
const IMPLEMENT_PLAN = "Implement the plan above. Work through the checklist in order.";

/** Why the open chat cannot be left: its turn would land in the next one. */
const STAY = "Stop the running turn before leaving this chat";

export default function App() {
  // Laid out as it was left.
  const [collapsed, setCollapsed] = useStoredState("atlas-sidebar-collapsed", false, isBoolean);
  // Agent works in the open folder; Chat only talks. Chat's conversation lives
  // here rather than in its panel, so a reply keeps arriving while Agent is shown.
  const [mode, setMode] = useStoredState<AppMode>("atlas-mode", "agent", isAppMode);
  const agentMode = mode === "agent";
  const plain = usePlainChat();
  // Hidden until the chat header's button asks for it.
  const [asideHidden, setAsideHidden] = useStoredState("atlas-aside-hidden", true, isBoolean);
  const [tab, setTab] = useStoredState<AsideTab>("atlas-aside-tab", "changes", isPaneIn("right"));
  // The strip under the chat: closed when null.
  const [bottomTab, setBottomTab] = useStoredState<AsideTab | null>("atlas-bottom-tab", null, isBottomTab);
  // Held here, not in the Changes pane: closing the pane, or moving it to the
  // other dock, would otherwise throw away a message half written. A different
  // folder is a different repository, and starts empty.
  const [commitMessage, setCommitMessage] = useState("");
  // Which background process a chat row asked the Terminal tab to show. The
  // row and the tab are in different subtrees, so it passes through here.
  const [processFocus, setProcessFocus] = useState<{ id: number } | null>(null);
  const [agentFocus, setAgentFocus] = useState<AgentFocus | null>(null);
  // A shell block an answer asked to put in a shell, until the Terminal tab
  // takes it. Kept only till then: an ask still held when the tab is drawn
  // again would be pasted again. `pasteInTerminal` is stable, so the answers'
  // Markdown does not re-render with every App render.
  const [terminalPaste, setTerminalPaste] = useState<{ command: string } | null>(null);
  const pasteInTerminal = useCallback((command: string) => setTerminalPaste({ command }), []);
  const terminalPasted = useCallback(() => setTerminalPaste(null), []);
  // A terminal selection on its way to the composer, from the other subtree.
  const [quote, setQuote] = useState<{ text: string; seq: number } | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Bumped to put the cursor in the message box, which lives in another subtree.
  const [composerFocus, setComposerFocus] = useState(0);
  const focusComposer = () => setComposerFocus((n) => n + 1);
  const [logOpen, setLogOpen] = useState(false);
  // The MCP and hooks files open as a whole (JSON) or one entry at a time
  // (a form): `entry` is the server's name or the hook's row, null for a new one.
  const [mcpDialog, setMcpDialog] = useState<EntryDialog<string>>(null);
  const [hooksDialog, setHooksDialog] = useState<EntryDialog<number>>(null);
  const mcpEditing = mcpDialog !== null;
  const hooksEditing = hooksDialog !== null;
  // What the agent may do this turn, and whether anyone is asked before it
  // does it. Two chips, two questions — and both are enforced on the backend,
  // so these hold only what the chips read back.
  const conversation = useBackendSetting(setConversationMode, "agent" as ConversationMode);
  const toast = useToast();
  const workspace = useWorkspace();
  const index = useIndexStatus(workspace.path);
  useEffect(() => setCommitMessage(""), [workspace.path]);
  // The files open in the viewer beside the chat, from Changes or Files.
  const viewer = useOpenFiles(workspace.path);
  const openFileLink = useCallback(
    (link: string) => {
      const path = workspace.path && fileLinkPath(link, workspace.path);
      if (path) viewer.open({ path, side: "worktree" });
    },
    [workspace.path, viewer.open],
  );
  // On unless turned off in Settings: the viewer is a column beside the chat,
  // rarely wide enough for a long line.
  const [wrapLines, setWrapLines] = useStoredState("viewer-wrap", true, isBoolean);
  // A window too narrow for every open panel at its minimum: the sidebar
  // falls back to its rail first, then the column right of the viewer hides.
  // Neither is stored — wider again, both come back as they were left.
  const frame = nativeFrame ? 0 : 2;
  // Chat mode has no folder to show: the viewer and the panes beside the chat stay out of it.
  const viewerOpen = agentMode && viewer.active !== null;
  const dockFits = useMediaQuery(`(min-width: ${roomFor({ rail: true, viewer: viewerOpen, dock: true, frame })}px)`);
  // What the column shows: what was opened, while it fits.
  const topHidden = asideHidden || !dockFits || !agentMode;
  const bottomShown = dockFits && agentMode ? bottomTab : null;
  const dockShown = !topHidden || bottomShown !== null;
  const sidebarFits = useMediaQuery(`(min-width: ${roomFor({ rail: false, viewer: viewerOpen, dock: dockShown, frame })}px)`);
  const rail = collapsed || !sidebarFits;
  // On screen in either dock: what decides whether a pane's data is read.
  const shown = (pane: AsideTab) => (tab === pane && !topHidden) || bottomShown === pane;
  const toolLog = useToolLog(logOpen);
  const history = useChatHistory(workspace.path);
  // The list is redrawn from disk after every save rather than guessed at
  // here: what belongs in it, and in what order, is the store's rule.
  // Queued messages that will not be sent after all go back to the box.
  // Counted in the chat beside the turn's clock: they outlive the turn.
  const runningProcesses = useRunningProcesses();
  const agent = useAgentTurn({
    onSaved: history.refresh,
    onGiveBack: (text) => setQuote((last) => ({ text, seq: (last?.seq ?? 0) + 1 })),
  });
  const approval = useApprovalMemory(agent.chatId, workspace.path, () =>
    toast.show("Auto is on here — the agent will change files without asking"),
  );
  const branch = useGitBranch(workspace.path, agent.turn.status);
  const worktreeOf = useWorktreeOf(workspace.path);
  const changeTotals = useChangeTotals(workspace.path);
  // An archived chat is not where the user left off, even when it was touched last.
  useFolderConversation(workspace.path, workspace.resumed, history.chats.find((one) => !one.archived)?.id, agent);
  // Sounds and notifications when the agent finishes or asks, each switched in Settings.
  const [alerts, setAlerts] = useStoredState("turn-alerts", DEFAULT_ALERTS, isAlertPrefs);
  useTurnAlerts({
    status: agent.turn.status,
    failed: agent.error !== null,
    queued: agent.queued.length,
    // A chat's first turn ends before it is saved and named: its first message is its name.
    chatTitle:
      history.chats.find((one) => one.id === agent.chatId)?.title ??
      agent.turn.blocks.find((block) => block.kind === "user")?.text ??
      null,
    prefs: alerts,
  });
  // Servers start with an Agent turn and may stop during one.
  // Settings shows both in "Where your data goes".
  const mcp = useMcp(shown("mcp") || mcpEditing || settingsOpen, agent.turn.status);
  const hooks = useHooks(shown("hooks") || hooksEditing || settingsOpen);
  // Settings has its own copy: which skills folders are read. The Skills
  // pane reads the same list for itself while it is open.
  const skillSources = useSkills(settingsOpen, workspace.path);
  const commandFiles = useCommandFiles(workspace.path);
  const llm = useLlmSettings();
  const theme = useTheme();
  const fontSize = useChatFontSize();
  const panels = usePanelSizes({
    sidebar: {
      collapsed: rail,
      collapse: () => setCollapsed(true),
      expand: () => setCollapsed(false),
    },
  });

  // Where each pane goes is `lib/docks.ts`'s rule; this only stores the answer.
  const docks: Docks = { top: tab, topHidden, bottom: bottomShown };
  const setDocks = (next: Docks) => {
    // A pane asked for where the viewer leaves the column no room: the viewer
    // gives way, or the pane would open out of sight.
    if (!dockFits && (!next.topHidden || next.bottom)) viewer.closeAll();
    setTab(next.top);
    setAsideHidden(next.topHidden);
    setBottomTab(next.bottom);
  };
  const openTab = (next: AsideTab) =>
    setDocks(openPane(docks, next, PANES.find((p) => p.id === next)?.dock ?? "right"));
  // A command sent to a shell shows the shell.
  useEffect(() => {
    if (terminalPaste) openTab("terminal");
    // Once per ask: not again when the layout changes.
  }, [terminalPaste]);

  // Held on its rail by the viewer beside the chat: opening it closes the viewer.
  const toggleSidebar = () => (rail && !collapsed ? viewer.closeAll() : setCollapsed((v) => !v));

  // A pane's key shows it, or hides it where it already is (`lib/shortcuts.ts`).
  useShortcuts({
    ...Object.fromEntries(
      PANES.map(({ id, dock }) => [
        id,
        () => {
          // Focus leaves the composer, so the shell that opens takes the typing.
          if (id === "terminal") (document.activeElement as HTMLElement | null)?.blur();
          setDocks(togglePane(docks, id, dock));
        },
      ]),
    ),
    newChat: () => newChat(),
    focusInput: focusComposer,
    sidebar: toggleSidebar,
    settings: () => setSettingsOpen(true),
    // Not ⌘W: once the last tab is gone, the next press would close the window.
    closeFile: viewer.active ? () => viewer.active && viewer.close(viewer.active) : undefined,
  });

  // A plan the agent has just finished writing is shown, once, when its turn
  // ends — not mid-turn, while it is still filling in the checklist.
  useEffect(() => {
    if (agent.planWritten > 0) openTab("plan");
  }, [agent.planWritten]);

  // The conversation just left is already on disk and stays in the sidebar;
  // this only stops pointing at it.
  const newChat = () => {
    if (!agentMode) {
      plain.clear();
      return focusComposer();
    }
    if (!agent.reset()) return toast.show(STAY);
    focusComposer();
  };

  // The window the meter is drawn against, and the provider a turn talks to.
  const compactNow = async () => {
    if (!(await agent.compact(true))) {
      toast.show(agent.error ?? "Nothing worth folding away yet");
    }
  };

  // What `/name` in the composer runs: the app's own, then the user's files
  // in `.kibo/commands`. The composer takes whatever is in the list.
  const busy = agent.turn.status === "running" || agent.turn.status === "awaitingApproval";
  const builtIn: SlashCommand[] = [
    {
      name: "compact",
      hint: "Fold older history into a summary to free the context window",
      unavailable: busy ? "Not while a turn is running" : undefined,
      run: () => void compactNow(),
    },
    {
      name: "fork",
      hint: "Continue in a new chat with a copy of this conversation",
      unavailable: busy
        ? "Not while a turn is running"
        : agent.turn.blocks.length === 0
          ? "Nothing to fork yet"
          : undefined,
      run: () => agent.branch(),
    },
    {
      name: "review",
      hint: "Review the uncommitted changes for bugs, and list what it finds",
      unavailable: busy ? "Not while a turn is running" : !workspace.path ? "Open a folder first" : undefined,
      run: () => void agent.review(),
    },
    {
      name: "init",
      hint: "Write AGENTS.md and CLAUDE.md for this repository, or suggest edits to the ones it has",
      argumentHint: "[what to focus on]",
      unavailable: busy
        ? "Not while a turn is running"
        : conversation.value !== "agent"
          ? "Switch to Agent mode — it writes files"
          : undefined,
      run: (args) => void send(typedCommand("init", args), expandTemplate(initPrompt, args)),
    },
  ];
  // `send` is declared further down; the arrow reaches it when a command runs.
  const commands = [...builtIn, ...fileCommands(commandFiles.files, builtIn, (text, sent) => void send(text, sent))];

  const pickConversation = async (mode: ConversationMode) => {
    const failed = await conversation.pick(mode);
    if (failed) toast.show(failed);
  };

  // Said out loud in one direction only. Turning confirmations back on needs
  // no warning; turning them off means the next write happens without anyone
  // seeing it, and the chip alone is a small thing to have noticed.
  const pickUnattended = async (next: boolean) => {
    const failed = await approval.pick(next);
    if (failed) toast.show(failed);
    else if (next) toast.show("Auto — the agent will change files without asking");
  };

  // Read from state rather than after the `await`: the closure there still
  // holds the render before the failure.
  useEffect(() => {
    if (workspace.error) toast.show(workspace.error);
  }, [workspace.error]);

  // Leaving the folder ends what runs in it: the user is told first.
  const folderSwitch = useFolderSwitch(
    agent.turn.status === "running" ? "running" : agent.turn.status === "awaitingApproval" ? "waiting" : null,
  );
  const openFolder = (path: string) => folderSwitch.guard(() => void workspace.open(path));
  const chooseFolder = () => folderSwitch.guard(() => void workspace.pick());
  // Offered only before the first message: a chat that has started is about
  // the branch it started on.
  const branchPicker = useBranchPicker({
    current: branch,
    guard: folderSwitch.guard,
    open: workspace.open,
    notify: toast.show,
    send: agent.send,
  });
  const unstarted = agent.turn.blocks.length === 0 && agent.turn.status !== "running";
  const worktreeRemoval = useWorktreeRemoval({ notify: toast.show, refreshRecent: workspace.refreshRecent });
  const rewinding = useRewind({ preview: agent.previewRewind, rewind: agent.rewind, notify: toast.show });
  // The composer cleared the box when the message went; one that is not sent
  // after all is put back rather than typed again.
  const giveBack = (text: string) => setQuote((last) => ({ text, seq: (last?.seq ?? 0) + 1 }));

  // Asking before the first message rather than refusing it — and then sending
  // it: the composer has already cleared the box, so anything not sent here is
  // typed twice.
  // `command`, when given, is what the model gets in place of `text` — a `/`
  // command's prompt — with the reply language said at its end.
  const send = async (text: string, command?: string) => {
    const sent = command === undefined ? undefined : withLanguageReminder(command, llm.settings?.replyLanguage ?? "auto");
    if (!workspace.path && !(await workspace.pick())) return;
    // With Worktree ticked the first message is where the worktree is made:
    // sent here, it would be worked on in the folder the user meant to keep out of.
    if (unstarted && branchPicker.worktree && branchPicker.base) {
      if (!(await branchPicker.startWorktree(branchPicker.base, text, sent))) giveBack(text);
      return;
    }
    agent.send(text, sent);
  };

  const openChat = history.chats.find((one) => one.id === agent.chatId);

  // The conversation as a file, for reading it somewhere else. Only what is
  // on disk can be written out — a turn saves when it comes to rest, so this
  // exports everything up to the one still running.
  const exportOpenChat = async () => {
    const chat = openChat;
    if (!chat) return toast.show("Nothing saved to export yet");
    const path = await pickSavePath(chat.title, "md");
    if (!path) return;
    try {
      await exportChat(chat.id, path);
      toast.show(`Exported to ${path.split("/").pop()}`);
    } catch (e) {
      toast.show(String(e));
    }
  };

  // The mode first, and only then the message: the backend reads the mode
  // when the turn starts, and a turn sent a moment early would still be a
  // plan that cannot write.
  const implement = async () => {
    const failed = await conversation.pick("agent");
    if (failed) return toast.show(failed);
    agent.send(IMPLEMENT_PLAN);
  };

  // What every pane is drawn from, whichever dock it sits in.
  const panes: Omit<PaneContext, "active"> = {
    workspace: workspace.path,
    onNotify: toast.show,
    commitDraft: { message: commitMessage, onMessage: setCommitMessage },
    processFocus,
    agentFocus,
    terminalPaste,
    onTerminalPasted: terminalPasted,
    onAddToChat: (text) => setQuote((last) => ({ text, seq: (last?.seq ?? 0) + 1 })),
    chatBlocks: agent.turn.blocks,
    openFile: viewer.active,
    onOpenFile: viewer.open,
    mcp: {
      view: mcp.view,
      error: mcpEditing ? null : mcp.error,
      onAdd: () => setMcpDialog({ entry: null }),
      onEditServer: (name) => setMcpDialog({ entry: name }),
      onRemoveServer: (name) => {
        const next = removeMcpServer(mcp.view?.text ?? "", name);
        if (next) void mcp.save(next);
      },
      onToggle: mcp.setEnabled,
      onOpen: mcp.connect,
      onEditFile: () => setMcpDialog("json"),
    },
    hooks: {
      view: hooks.view,
      error: hooksEditing ? null : hooks.error,
      onAdd: () => setHooksDialog({ entry: null }),
      onEditHook: (index) => setHooksDialog({ entry: index }),
      onRemoveHook: (index) => {
        const next = removeHook(hooks.view?.text ?? "", index);
        if (next) void hooks.save(next);
      },
      onEditFile: () => setHooksDialog("json"),
    },
    plan: {
      plan: agent.plan,
      checklist: agent.checklist,
      onEdit: agent.editPlan,
      onImplement: conversation.value === "plan" ? implement : undefined,
      locked: agent.turn.status === "running",
    },
  };

  return (
    <div
      className={`window${nativeFrame ? " native-frame" : ""}${rail ? " collapsed" : ""}${topHidden ? " aside-hidden" : ""}${bottomShown ? "" : " bottom-closed"}`}
      style={
        {
          "--sidebar-width": `${panels.widths.sidebar}px`,
          "--aside-width": `${panels.widths.aside}px`,
          "--bottom-height": `${panels.widths.bottom}px`,
          "--viewer-width": `${panels.widths.viewer}px`,
        } as React.CSSProperties
      }
    >
      <div className="titlebar" onMouseDown={dragOrMaximize}>
        <WindowControls />
        <span className="titlebar-title">
          Kibo{workspace.path && <span> · {workspace.path.split("/").pop()}</span>}
        </span>
      </div>

      <div className="body">
        <Sidebar
          mode={mode}
          onMode={setMode}
          chats={history.chats}
          activeChat={agentMode ? agent.chatId : null}
          onSelectChat={(id) => {
            setMode("agent");
            void agent.open(id).then((opened) => opened || toast.show(STAY));
          }}
          onNewChat={newChat}
          onArchiveChat={(id, archived) => history.archive(id, archived).catch((e) => toast.show(String(e)))}
          onDeleteChat={(id) => {
            // The open chat goes first: left on screen, its next save would write it back.
            if (id === agent.chatId && !agent.reset()) return toast.show(STAY);
            history.remove(id).catch((e) => toast.show(String(e)));
          }}
          onToggleCollapse={toggleSidebar}
          onOpenSettings={() => setSettingsOpen(true)}
          onOnboardingAction={openTab}
        />

        <PanelResizeHandle
          ariaLabel="Resize the chat list"
          onResize={panels.resizeSidebarBy}
          onResizeEnd={(size) => panels.endResize("sidebar", size)}
        />

        <main className="main">
          {agentMode ? (
            <>
              <ChatPanel
                title={openChat?.title ?? null}
                branched={Boolean(openChat?.branchedFrom)}
                workspace={workspace.path}
                turn={agent.turn}
                onDecide={agent.decide}
                onOpenRepo={chooseFolder}
                asideOpen={changesShown(docks)}
                onToggleAside={() => setDocks(toggleChanges(docks))}
                terminalOpen={bottomShown === "terminal"}
                onToggleTerminal={() => setDocks(toggleTerminal(docks))}
                onOpenPanel={openTab}
                onExport={exportOpenChat}
                onImplement={conversation.value === "plan" ? implement : undefined}
                onOpenPlan={() => openTab("plan")}
                onOpenProcess={(id) => {
                  openTab("terminal");
                  setProcessFocus({ id });
                }}
                onOpenAgent={(agent) => {
                  openTab("terminal");
                  setAgentFocus({ ...agent });
                }}
                runningProcesses={runningProcesses.map((process) => process.id)}
                onPasteCommand={workspace.path ? pasteInTerminal : undefined}
                onOpenFile={workspace.path ? openFileLink : undefined}
                branchable={agent.branchable}
                onBranch={agent.branch}
                onRewind={(id) => void rewinding.ask(id)}
                onFix={(text) => setQuote((last) => ({ text, seq: (last?.seq ?? 0) + 1 }))}
              />
              <Composer
                tab={
                  <FolderTab
                    path={workspace.path}
                    recent={workspace.recent}
                    onOpenFolder={openFolder}
                    onPickFolder={chooseFolder}
                    onRemoveWorktree={worktreeRemoval.ask}
                    branch={branch}
                    worktreeOf={worktreeOf}
                    index={index}
                    changes={changeTotals}
                    onOpenChanges={() => openTab("changes")}
                    branchPicker={
                      unstarted
                        ? {
                            branches: branchPicker.branches,
                            onOpen: branchPicker.load,
                            onPick: (name) => void branchPicker.pick(name),
                            worktree: branchPicker.worktree,
                            base: branchPicker.base,
                            onWorktree: branchPicker.setWorktree,
                          }
                        : undefined
                    }
                  />
                }
                onSend={send}
                onQueue={agent.queue}
                queued={agent.queued}
                onUnqueue={agent.unqueue}
                focus={composerFocus}
                onStop={agent.cancel}
                running={agent.turn.status === "running"}
                conversation={conversation.value}
                onConversation={pickConversation}
                unattended={approval.unattended}
                onUnattended={pickUnattended}
                draft={agent.draft}
                quote={quote}
                models={llm.models}
                onModel={(choice) => llm.pickModel(choice.providerId, choice.model)}
                onEffort={llm.pickEffort}
                onLoadModels={llm.loadModels}
                context={agent.context}
                usage={agent.turn.usage}
                onCompact={compactNow}
                commands={commands}
                onCommandsOpen={commandFiles.reload}
              />
            </>
          ) : (
            <PlainChat chat={plain} focus={composerFocus} />
          )}
        </main>

        {agentMode && viewer.active && (
          <>
            <PanelResizeHandle
              invert
              ariaLabel="Resize the file viewer"
              onResize={panels.resizeViewerBy}
              onResizeEnd={(size) => panels.endResize("viewer", size)}
            />
            <FileViewer
              files={viewer.files}
              active={viewer.active}
              preview={viewer.preview}
              workspace={workspace.path}
              onActivate={viewer.open}
              onPin={viewer.pin}
              onClose={viewer.close}
              onCloseAll={viewer.closeAll}
              wrap={wrapLines}
            />
          </>
        )}

        {dockShown && (
          <PanelResizeHandle
            invert
            ariaLabel="Resize the side panel"
            onResize={panels.resizeAsideBy}
            onResizeEnd={(size) => panels.endResize("aside", size)}
          />
        )}

        {/* The column right of the chat: the pane from the header's button on
            top, the bottom dock under it. Either may be closed; the column
            goes when both are. */}
        <div className="dock-column">
          <AsidePanel
            tab={tab}
            dock="right"
            ctx={{ ...panes, active: !topHidden }}
            onClose={() => setAsideHidden(true)}
          />
          {bottomShown && (
            <>
              {!topHidden && (
                <PanelResizeHandle
                  axis="y"
                  invert
                  ariaLabel="Resize the bottom panel"
                  onResize={panels.resizeBottomBy}
                  onResizeEnd={(size) => panels.endResize("bottom", size)}
                />
              )}
              <AsidePanel
                tab={bottomShown}
                dock="bottom"
                ctx={{ ...panes, active: true }}
                onClose={() => setBottomTab(null)}
              />
            </>
          )}
        </div>
      </div>

      <FolderSwitchDialog
        blocked={folderSwitch.blocked}
        folder={workspace.path}
        onStopAgent={agent.cancel}
        onClose={folderSwitch.close}
      />
      <RewindDialog asked={rewinding.asked} onConfirm={() => void rewinding.confirm()} onClose={rewinding.close} />
      <WorktreeRemoveDialog
        asked={worktreeRemoval.asked}
        onConfirm={() => void worktreeRemoval.confirm()}
        onClose={worktreeRemoval.close}
      />
      <BranchConflictDialog
        conflict={branchPicker.conflict}
        onWorktree={branchPicker.startWorktree}
        onClose={branchPicker.closeConflict}
      />

      <Modal title="Settings" wide open={settingsOpen} onClose={() => setSettingsOpen(false)}>
        <Settings
          skills={{ view: skillSources.view, error: skillSources.error, onToggle: skillSources.setSourceEnabled }}
          provider={{
            settings: llm.settings,
            busy: llm.busy,
            error: llm.error,
            onSave: llm.save,
            onRemove: llm.remove,
            onSelect: llm.select,
            onProbe: llm.probe,
            served: llm.served,
          }}
          remember={approval.remember}
          onRemember={async (next) => {
            const failed = await approval.pickRemember(next);
            if (failed) toast.show(failed);
          }}
          debugLogging={llm.settings?.debugLogging ?? false}
          onDebugLogging={llm.debugLogging}
          replyLanguage={llm.settings?.replyLanguage ?? "auto"}
          onReplyLanguage={llm.replyLanguage}
          theme={theme.choice}
          onThemeMode={theme.setMode}
          onThemePalette={theme.setPalette}
          fontSize={fontSize.size}
          onFontSize={fontSize.setSize}
          wrapLines={wrapLines}
          onWrapLines={setWrapLines}
          alerts={alerts}
          onAlerts={(next) => {
            // A sound is heard as it is turned on, so the user knows what to listen for.
            const turnedOn = (["done", "failed"] as const).find((kind) => next.sound[kind] && !alerts.sound[kind]);
            if (turnedOn) playSound(turnedOn === "done" ? "done" : "attention").catch(() => {});
            setAlerts(next);
          }}
          onOpenLog={() => {
            setSettingsOpen(false);
            setLogOpen(true);
          }}
          policy={{
            provider: llm.settings?.providers.find((p) => p.id === llm.settings?.activeProviderId) ?? null,
            debugLogging: llm.settings?.debugLogging ?? false,
            mcpServers: mcp.view?.servers ?? null,
            hooks: hooks.view?.hooks ?? null,
          }}
        />
      </Modal>

      <Modal title="Tool calls" wide open={logOpen} onClose={() => setLogOpen(false)}>
        <ToolLog
          rows={toolLog.rows}
          total={toolLog.total}
          filter={toolLog.filter}
          onFilter={toolLog.setFilter}
          onMore={toolLog.more}
          onClear={toolLog.clear}
          enabled={toolLog.enabled}
          onToggle={toolLog.toggle}
          error={toolLog.error}
        />
      </Modal>

      <Modal
        title={mcpDialog === "json" || mcpDialog === null ? "MCP servers" : mcpDialog.entry ?? "Add an MCP server"}
        wide={mcpDialog === "json"}
        open={mcpEditing}
        onClose={() => setMcpDialog(null)}
      >
        {mcpDialog !== null && mcpDialog !== "json" ? (
          <McpServerForm
            key={mcpDialog.entry ?? ""}
            name={mcpDialog.entry}
            text={mcp.view?.text}
            error={mcp.error}
            onSave={mcp.save}
            onClose={() => setMcpDialog(null)}
            onEditJson={() => setMcpDialog("json")}
          />
        ) : (
        <ConfigFileEditor
          label="MCP configuration"
          merge={mergeMcp}
          text={mcp.view?.text}
          error={mcp.error}
          onSave={mcp.save}
          onClose={() => setMcpDialog(null)}
          note={
            <>
              The <code>mcpServers</code> format of Claude Desktop and Cursor: paste a server's snippet as it is.
              Optional per server: <code>weight</code> (cost of a call in the turn's budget, 3 by default) and{" "}
              <code>timeoutSecs</code> (120). Kept in {mcp.view?.path || "the app directory"}, readable only by you —
              it may hold tokens.
            </>
          }
        />
        )}
      </Modal>

      <Modal
        title={hooksDialog === "json" || hooksDialog === null ? "Hooks" : hooksDialog.entry === null ? "Add a hook" : "Edit hook"}
        wide={hooksDialog === "json"}
        open={hooksEditing}
        onClose={() => setHooksDialog(null)}
      >
        {hooksDialog !== null && hooksDialog !== "json" ? (
          <HookForm
            key={hooksDialog.entry ?? -1}
            index={hooksDialog.entry}
            text={hooks.view?.text}
            error={hooks.error}
            onSave={hooks.save}
            onClose={() => setHooksDialog(null)}
            onEditJson={() => setHooksDialog("json")}
          />
        ) : (
        <ConfigFileEditor
          label="Hooks configuration"
          merge={mergeHooks}
          text={hooks.view?.text}
          error={hooks.error}
          onSave={hooks.save}
          onClose={() => setHooksDialog(null)}
          note={
            <>
              Claude Code's <code>hooks</code> format: <code>PreToolUse</code>, <code>PostToolUse</code> and{" "}
              <code>Stop</code> run here. The command gets the event as JSON on stdin; exit code 2 refuses the call,
              or sends the agent back from Stop, with stderr as the reason. A <code>matcher</code> names this app's
              tools — <code>runCommand</code>, <code>editFile</code> — not Claude Code's. Kept in{" "}
              {hooks.view?.path || "the app directory"}.
            </>
          }
        />
        )}
      </Modal>

      <Toast message={toast.message} />
    </div>
  );
}
