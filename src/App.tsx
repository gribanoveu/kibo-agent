import { Fragment, useCallback, useEffect, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatPanel } from "./components/ChatPanel";
import { AgentSheet } from "./components/AgentSheet";
import { PlanSheet } from "./components/PlanSheet";
import { Composer } from "./components/Composer";
import { ProviderBalance } from "./components/ProviderBalance";
import { PlainChat } from "./components/PlainChat";
import { FolderTab } from "./components/FolderTab";
import { AsidePanel } from "./components/AsidePanel";
import { PrototypePreview } from "./components/PrototypePreview";
import { FileViewer } from "./components/FileViewer";
import { PANES, type PaneContext } from "./components/panes";
import { Modal } from "./components/Modal";
import { type SettingsSection } from "./components/Settings";
import { SettingsDialog } from "./components/SettingsDialog";
import { ToolLog } from "./components/ToolLog";
import { McpConfigDialog } from "./components/McpConfigDialog";
import { HooksConfigDialog } from "./components/HooksConfigDialog";
import { PanelResizeHandle } from "./components/PanelResizeHandle";
import { Columns } from "./components/Columns";
import { pagesShown, type PageDef } from "./components/pages";
import { useFeatures } from "./hooks/useFeatures";
import { usePages } from "./hooks/usePages";
import { useDocks } from "./hooks/useDocks";
import { useConfigDialogs } from "./hooks/useConfigDialogs";
import { STAY, useSidebarActions } from "./hooks/useSidebarActions";
import { useChatRouting } from "./hooks/useChatRouting";
import { useComposerCommands } from "./hooks/useComposerCommands";
import { ChatSwitcher } from "./components/ChatSwitcher";
import { Toast } from "./components/Toast";
import { PanelLeft, SquarePen } from "lucide-react";
import { Titlebar } from "./components/Titlebar";
import { shortcutText } from "./lib/shortcuts";
import { useAgentTurn } from "./hooks/useAgentTurn";
import { usePlainChat } from "./hooks/usePlainChat";
import { usePrototypePreview } from "./hooks/usePrototypePreview";
import { useChatHistory } from "./hooks/useChatHistory";
import { useChatGroups } from "./hooks/useChatGroups";
import { useLlmSettings } from "./hooks/useLlmSettings";
import { useKubeconfigs } from "./hooks/useKubeconfigs";
import { useWorkspace } from "./hooks/useWorkspace";
import { useIndexStatus } from "./hooks/useIndexStatus";
import { useMcp } from "./hooks/useMcp";
import { useHooks } from "./hooks/useHooks";
import { useSkills } from "./hooks/useSkills";
import { useToolLog } from "./hooks/useToolLog";
import { usePanelSizes } from "./hooks/usePanelSizes";
import { sideOf, useTheme } from "./hooks/useTheme";
import { useChatFontSize } from "./hooks/useChatFontSize";
import { useGitBranch, useWorktreeOf } from "./hooks/useGitBranch";
import { useChangeTotals } from "./hooks/useChangeTotals";
import { useToast } from "./hooks/useToast";
import { nativeFrame, windowsFrame } from "./lib/window";
import { useBackendSetting } from "./hooks/useBackendSetting";
import { useApprovalMemory } from "./hooks/useApprovalMemory";
import { useFolderConversation } from "./hooks/useFolderConversation";
import { chatDraftKey, forgetChatDraft } from "./hooks/useChatDraft";
import { useTurnAlerts } from "./hooks/useTurnAlerts";
import { useRunningProcesses } from "./hooks/useProcesses";
import { DEFAULT_ALERTS, isAlertPrefs } from "./lib/attention";
import { isBoolean, useStoredState } from "./hooks/useStoredState";
import { removeHook, removeMcpServer } from "./lib/configEntries";
import { togglePane, toggleTerminal } from "./lib/docks";
import { useShortcuts } from "./hooks/useShortcuts";
import { type ConversationMode, getConversationMode, previewCalls, setConversationMode } from "./lib/chat";
import type { ImagePart } from "./lib/images";
import { type ChatSummary } from "./lib/savedChats";
import { DEFAULT_TURN_LIMITS } from "./lib/settings";
import type { TurnState } from "./lib/chatTurnReducer";
import { isAppMode, type AppMode } from "./types";
import { turnsIn, useFolderSwitch } from "./hooks/useFolderSwitch";
import { fileLink, useOpenFiles } from "./hooks/useOpenFiles";
import { FolderSwitchDialog } from "./components/FolderSwitchDialog";
import { FolderTrustDialog } from "./components/FolderTrustDialog";
import { useFolderTrust } from "./hooks/useFolderTrust";
import { useBranchPicker } from "./hooks/useBranchPicker";
import { BranchConflictDialog } from "./components/BranchConflictDialog";
import { useWorktreeRemoval } from "./hooks/useWorktreeRemoval";
import { WorktreeRemoveDialog } from "./components/WorktreeRemoveDialog";
import { RewindDialog } from "./components/RewindDialog";
import { useRewind } from "./hooks/useRewind";
import { useCommandFiles } from "./hooks/useCommandFiles";
import { useMcpPrompts } from "./hooks/useMcpPrompts";
import "./App.css";
import type { AgentFocus } from "./lib/describeTool";

/** The open chat's name. A chat's first turn ends before it is saved and named: its first message is its name. */
function chatName(chats: ChatSummary[], id: string | null, turn: TurnState): string | null {
  return chats.find((one) => one.id === id)?.title ?? turn.blocks.find((block) => block.kind === "user")?.text ?? null;
}

export default function App() {
  // Agent works in the open folder; Chat only talks. Chat's conversations live
  // here rather than in its panel: the sidebar lists them, and a reply keeps
  // arriving while Agent is shown.
  const [mode, setMode] = useStoredState<AppMode>("atlas-mode", "agent", isAppMode);
  const agentMode = mode === "agent";
  // Files left, the file in the middle, the chat right: for reading and writing documents.
  const [ideLayout, setIdeLayout] = useStoredState("atlas-ide-layout", false, isBoolean);
  const ide = agentMode && ideLayout;
  // A sidebar page in the chat's place; picking a mode or a chat leaves it.
  const features = useFeatures();
  const pages = pagesShown(features.isOn);
  const { onPage, pageItem, keptPages, openPage, leavePage } = usePages(ide, pages);
  // Before Chat mode: a new chat starts at the kubeconfig picked last.
  const kube = useKubeconfigs();
  const llm = useLlmSettings();
  // The model turns run on, for saying a cache miss came with a switch.
  const model = llm.models.current?.label;
  const plain = usePlainChat(kube.active?.name ?? null, model, !agentMode && !onPage);
  const prototypePreview = usePrototypePreview(plain.chatId, plain.role === "prototype" ? plain.turn.blocks : null, plain.turn.status === "running");
  // Held here, not in the Changes pane: closing the pane, or moving it to the
  // other dock, would otherwise throw away a message half written. A different
  // folder is a different repository, and starts empty.
  const [commitMessage, setCommitMessage] = useState("");
  // The title bar's room for the chat's header, once it is on screen.
  const [headSlot, setHeadSlot] = useState<HTMLDivElement | null>(null);
  // Which background process a chat row asked the Terminal tab to show — `null`
  // for the list. The row and the tab are in different subtrees, so it passes through here.
  const [processFocus, setProcessFocus] = useState<{ id: number | null } | null>(null);
  // The helper run opened in a sheet over the chat, with the chat it is from:
  // another chat's sheet is not drawn over this one.
  const [agentShown, setAgentShown] = useState<{ run: AgentFocus; chatId: string | null } | null>(null);
  const closeAgent = useCallback(() => setAgentShown(null), []);
  // A shell block an answer asked to put in a shell, until the Terminal tab
  // takes it. Kept only till then: an ask still held when the tab is drawn
  // again would be pasted again. `pasteInTerminal` is stable, so the answers'
  // Markdown does not re-render with every App render.
  const [terminalPaste, setTerminalPaste] = useState<{ command: string } | null>(null);
  const pasteInTerminal = useCallback((command: string) => setTerminalPaste({ command }), []);
  const terminalPasted = useCallback(() => setTerminalPaste(null), []);
  // A terminal selection on its way to the composer, from the other subtree —
  // and pictures a note carried back out of a running turn.
  const [quote, setQuote] = useState<{ text: string; seq: number; images?: ImagePart[] } | null>(null);
  // Text put into the box: a message not sent after all (the composer had
  // cleared the box), a terminal selection, a fix to ask for — with the
  // pictures that were going to be sent with it.
  const giveBack = (text: string, images: ImagePart[] = []) => setQuote((last) => ({ text, images, seq: (last?.seq ?? 0) + 1 }));
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Which section it opens at: Models, unless something sent the user elsewhere.
  const [settingsAt, setSettingsAt] = useState<SettingsSection | undefined>();
  const openSettings = (at?: SettingsSection) => {
    setSettingsAt(at);
    setSettingsOpen(true);
  };
  // Bumped to put the cursor in the message box, which lives in another subtree.
  const [composerFocus, setComposerFocus] = useState(0);
  const focusComposer = () => setComposerFocus((n) => n + 1);
  const [logOpen, setLogOpen] = useState(false);
  // The MCP and hooks dialogs: a form for one entry, or the whole file as JSON.
  const { mcpDialog, hooksDialog, mcpEditing, hooksEditing, addMcp, editMcp, editMcpFile, closeMcp, addHook, editHook, editHooksFile, closeHooks } = useConfigDialogs();
  // What the agent may do this turn, and whether anyone is asked before it
  // does it. Two chips, two questions — and both are enforced on the backend,
  // so these hold only what the chips read back.
  const conversation = useBackendSetting(setConversationMode, "agent" as ConversationMode, getConversationMode);
  const toast = useToast();
  const workspace = useWorkspace();
  const index = useIndexStatus(workspace.path);
  useEffect(() => setCommitMessage(""), [workspace.path]);
  // The files open in the viewer beside the chat, from Changes or Files.
  const viewer = useOpenFiles(workspace.path);
  const openFileLink = useCallback(
    (link: string) => {
      const place = workspace.path && fileLink(link, workspace.path);
      if (place) viewer.openAt(place);
    },
    [workspace.path, viewer.openAt],
  );
  // On unless turned off in Settings: the viewer is a column beside the chat,
  // rarely wide enough for a long line.
  const [wrapLines, setWrapLines] = useStoredState("viewer-wrap", true, isBoolean);
  // Chat mode has no folder to show: the panes beside the chat stay out of
  // it, and the viewer's place holds a Prototype chat's preview — a page
  // wants the width of a file, not of a side panel.
  const previewShown = !agentMode && !onPage && prototypePreview.open && plain.chatId !== null;
  const viewerOpen = (agentMode && !onPage && viewer.active !== null) || previewShown;
  // The layout: what fits the window, what is shown, and the panes' own keys.
  const { rail, tab, topHidden, bottomShown, planSheet, dockShown, docks, setDocks, openTab, toggleIde, toggleSidebar, shown, setCollapsed, hideAside, closeBottom } = useDocks({
    agentMode,
    ideLayout,
    setIdeLayout,
    page: onPage,
    panes: PANES,
    viewerOpen,
    closeViewer: viewer.closeAll,
    swapViewer: viewer.swap,
    terminalPaste,
  });
  const toolLog = useToolLog(logOpen);
  const history = useChatHistory(workspace.folders, workspace.path);
  const chatGroups = useChatGroups(agentMode ? "projects" : "chat");
  // The list is redrawn from disk after every save rather than guessed at
  // here: what belongs in it, and in what order, is the store's rule.
  // Queued messages that will not be sent after all go back to the box.
  const agent = useAgentTurn({
    model,
    folder: workspace.path,
    onSaved: history.refresh,
    onGiveBack: giveBack,
    spentShown: agentMode && shown("overview"),
  });
  // Counted in the chat beside the turn's clock: they outlive the turn.
  const runningProcesses = useRunningProcesses(agent.chatId);
  const approval = useApprovalMemory(agent.chatId, workspace.path, () =>
    toast.show("Auto is on here — the agent will change files without asking"),
  );
  const branch = useGitBranch(workspace.path, agent.turn.status);
  const worktreeOf = useWorktreeOf(workspace.path);
  const changeTotals = useChangeTotals(workspace.path);
  // A turn keeps its own folder while the window moves (F-8.7): only closing
  // that folder, or switching its branch, waits for it.
  const turnsInFolder = turnsIn(agent.chatActivity, history.folderOf);
  const folderSwitch = useFolderSwitch(turnsInFolder);
  // Getting a chat on screen and a message out of the box.
  const { pendingChat, pendingChatOpened, selectChat, newChat, newChatIn, send, pickOffer, implement, exportOpenChat, compactNow } = useChatRouting({
    agentMode,
    agent,
    plain,
    history,
    workspace,
    folderSwitch,
    conversation,
    replyLanguage: llm.settings?.replyLanguage ?? "auto",
    notify: toast.show,
    focusComposer,
    leavePage,
    giveBack,
    worktreeMain: worktreeOf,
    // Declared below; the closure runs on a press, long after this render.
    askWorktreeRemoval: (path) => worktreeRemoval.ask(path),
  });
  // An archived chat is not where the user left off, even when it was touched last.
  useFolderConversation(
    workspace.path,
    workspace.resumed,
    history.chats.find((one) => !one.archived)?.id,
    agent,
    pendingChat,
    pendingChatOpened,
  );
  // Sounds and notifications when the agent finishes or asks, each switched in Settings.
  const [alerts, setAlerts] = useStoredState("turn-alerts", DEFAULT_ALERTS, isAlertPrefs);
  // Every chat's, the ones left working in another chat or folder too.
  const everyChat = Object.values(history.byFolder).flat();
  useTurnAlerts({
    turns: Object.fromEntries(
      Object.entries(agent.turns).map(([key, one]) => [
        key,
        { status: one.turn.status, failed: one.failed, queued: one.queued, chatTitle: chatName(everyChat, key, one.turn) },
      ]),
    ),
    prefs: alerts,
  });
  // A turn with no model pinned pins the provider's first one
  // (llm_session::effective_model). The settings are read again as a turn
  // starts or ends, as the branch is, so the line names the model that went
  // out rather than the auto guess.
  useEffect(() => {
    void llm.reload();
  }, [agent.turn.status, plain.turn.status, llm.reload]);
  // Servers start with an Agent or Analyst turn and may stop during one.
  // Settings shows both in "Where your data goes".
  const mcp = useMcp(shown("mcp") || shown("overview") || mcpEditing || settingsOpen, workspace.path, agent.turn.status);
  const hooks = useHooks(shown("hooks") || hooksEditing || settingsOpen);
  // Settings has its own copy: which skills folders are read. The Skills
  // pane reads the same list for itself while it is open.
  const skillSources = useSkills(settingsOpen, workspace.path);
  const commandFiles = useCommandFiles(workspace.path);
  // The folder's own commands are listed only once it is trusted.
  const trust = useFolderTrust(workspace.path, commandFiles.reload);
  const serverPrompts = useMcpPrompts(workspace.path, agent.turn.status);
  const theme = useTheme();
  const fontSize = useChatFontSize();
  const panels = usePanelSizes({
    sidebar: {
      collapsed: rail,
      collapse: () => setCollapsed(true),
      expand: () => setCollapsed(false),
    },
  });

  // The window's own keys; a pane's key lives in `useDocks`.
  useShortcuts({
    newChat: () => newChat(),
    focusInput: focusComposer,
    sidebar: toggleSidebar,
    settings: () => openSettings(),
    ideLayout: agentMode ? toggleIde : undefined,
  });

  // A deleted chat takes its unsent message with it — the sidebar's own path
  // and the Artifacts page's prototype, which deletes its chat through the same
  // `plain.remove`.
  const dropDraft = (mode: "agent" | "chat", id: string) => forgetChatDraft(chatDraftKey(mode, id));
  const sidebarActions = useSidebarActions({ agentMode, agent, plain, history, groups: chatGroups, notify: toast.show, dropDraft });

  // What `/name` in the composer runs: the app's own, the user's files, the servers' prompts.
  const commands = useComposerCommands({
    agent,
    folder: workspace.path,
    agentMode: conversation.value === "agent",
    compact: () => void compactNow(),
    send,
    trust: trust.ask,
    commandFiles,
    serverPrompts,
    notify: toast.show,
  });

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

  // Opening a folder closes none (F-8.6); closing one stops what runs in it, and says so first.
  const openFolder = (path: string) => folderSwitch.guard(() => void workspace.open(path));
  const chooseFolder = () => folderSwitch.guard(() => void workspace.pick());
  const closeFolder = (path: string) => void folderSwitch.guard(() => void workspace.close(path), undefined, [path]);
  // Before the first message the branch is where the chat starts; after it,
  // the chat's own branch — switched only between turns, never under one.
  const branchPicker = useBranchPicker({
    workspace: workspace.path,
    current: branch,
    guard: folderSwitch.guard,
    open: workspace.open,
    notify: toast.show,
    onSwitched: agent.saveBranch,
  });
  // A turn working in the folder forbids switching its branch under it; the
  // menu still opens, so a worktree — the way round — stays reachable.
  const branchLocked =
    workspace.path && turnsInFolder(workspace.path).length > 0
      ? "An agent is working in this folder — wait for its turns to finish, or use Worktree for a new chat"
      : undefined;
  const unstarted = agent.turn.blocks.length === 0 && agent.turn.status !== "running";
  const turnBusy = agent.turn.status === "running" || agent.turn.status === "awaitingApproval";
  // The provider turns go to now: its own switch in Settings → Models.
  const imagesEnabled = !!llm.settings?.providers.find((p) => p.id === llm.models.current?.providerId)?.supportsImages;
  const worktreeRemoval = useWorktreeRemoval({ notify: toast.show, refreshRecent: workspace.refreshRecent });
  // One dialog for either chat: the one on screen is the one being rewound.
  const rewinding = useRewind(
    agentMode
      ? { preview: agent.previewRewind, rewind: agent.rewind, notify: toast.show }
      : { preview: plain.previewRewind, rewind: plain.rewind, notify: toast.show },
  );

  const openChat = history.chats.find((one) => one.id === agent.chatId);

  // A chat row or Overview asking to see one: Terminal shows it.
  const openProcess = (id: number | null) => {
    openTab("terminal");
    setProcessFocus({ id });
  };
  // A chat row, Overview or the Agents tab: the run opens over the chat.
  const openAgent = (run: AgentFocus) => setAgentShown({ run, chatId: agent.chatId });
  // Held only till Terminal takes it, as `terminalPaste` is: the pane is drawn
  // anew each time it opens, and an ask still held would be taken again.
  const focusTaken = useCallback(() => setProcessFocus(null), []);

  // What every pane is drawn from, whichever dock it sits in.
  const panes: Omit<PaneContext, "active"> = {
    workspace: workspace.path,
    ide,
    onNotify: toast.show,
    commitDraft: { message: commitMessage, onMessage: setCommitMessage },
    processFocus,
    chatId: agent.chatId,
    onOpenAgent: openAgent,
    onFocusTaken: focusTaken,
    terminalPaste,
    onTerminalPasted: terminalPasted,
    onAddToChat: giveBack,
    chatBlocks: agent.turn.blocks,
    openFile: viewer.active,
    onOpenFile: viewer.open,
    mcp: {
      view: mcp.view,
      error: mcpEditing ? null : mcp.error,
      onAdd: addMcp,
      onEditServer: editMcp,
      onRemoveServer: (name) => {
        const next = removeMcpServer(mcp.view?.text ?? "", name);
        if (next) void mcp.save(next);
      },
      onToggle: mcp.setEnabled,
      onToggleTool: mcp.setToolShown,
      onOpen: mcp.connect,
      onEditFile: editMcpFile,
    },
    hooks: {
      view: hooks.view,
      error: hooksEditing ? null : hooks.error,
      onAdd: addHook,
      onEditHook: editHook,
      onRemoveHook: (index) => {
        const next = removeHook(hooks.view?.text ?? "", index);
        if (next) void hooks.save(next);
      },
      onEditFile: editHooksFile,
    },
    overview: {
      branch,
      changes: changeTotals,
      context: agent.context,
      contextPending: agent.contextPending,
      usage: agent.turn.usage,
      spent: agent.spent,
      subagents: agent.subagents,
      onOpenProcess: openProcess,
      // Not `openTab`: Terminal would show whichever of its tabs was last picked.
      onOpenProcesses: () => openProcess(null),
      onOpenAgent: openAgent,
      onOpenPanel: openTab,
    },
    plan: {
      plan: agent.plan,
      checklist: agent.checklist,
      onEdit: agent.editPlan,
      // Not while a turn waits on a card either: the message would queue, and
      // a queued one drops the checklist.
      onImplement: turnBusy ? undefined : implement,
      locked: agent.turn.status === "running",
    },
  };

  // The terminal, under the message box: the only pane that opens there.
  const bottomPane = bottomShown && (
    <AsidePanel key="bottom" tab={bottomShown} dock="bottom" ctx={{ ...panes, active: true }} onClose={closeBottom} />
  );

  const chat = (
    <main key="chat" className="main">
      {agentMode ? (
        <>
          <ChatPanel
            // A page has the title bar to itself: the head stays in the hidden chat.
            headSlot={onPage ? null : headSlot}
            heading={
              ide ? (
                <ChatSwitcher title={openChat?.title ?? null} chats={history.chats} activeChat={agent.chatId} onSelect={selectChat} onNew={newChat} />
              ) : undefined
            }
            onOpenSettings={ide ? () => openSettings() : undefined}
            workspace={workspace.path}
            chatId={agent.chatId}
            turn={agent.turn}
            onDecide={agent.decide}
            onAnswerQuestion={agent.answerQuestion}
            onOpenRepo={chooseFolder}
            overviewOpen={shown("overview")}
            onToggleOverview={() => setDocks(togglePane(docks, "overview", "right"))}
            terminalOpen={bottomShown === "terminal"}
            onToggleTerminal={() => setDocks(toggleTerminal(docks))}
            ideLayout={ide}
            onToggleIde={toggleIde}
            onOpenPanel={openTab}
            onExport={exportOpenChat}
            onOpenPlan={() => openTab("plan")}
            checklist={agent.checklist}
            onOpenProcess={openProcess}
            onOpenAgent={openAgent}
            runningProcesses={runningProcesses.map((process) => process.id)}
            onPasteCommand={workspace.path ? pasteInTerminal : undefined}
            onOpenFile={workspace.path ? openFileLink : undefined}
            preview={(calls) => previewCalls(agent.chatId ?? "", calls)}
            branchable={agent.branchable}
            onBranch={agent.branch}
            onRewind={(id) => void rewinding.ask(id)}
            onFix={giveBack}
            onOffer={conversation.value === "agent" ? (kind) => void pickOffer(kind) : undefined}
            steered={agent.steered}
            onWithdraw={agent.withdraw}
          />
          <Composer
            tab={
              <FolderTab
                path={workspace.path}
                recent={workspace.recent}
                onOpenFolder={openFolder}
                onPickFolder={chooseFolder}
                onRemoveWorktree={worktreeRemoval.ask}
                onForgetFolder={(path) => void workspace.forget(path)}
                branch={branch}
                worktreeOf={worktreeOf}
                branchPicker={{
                  branches: branchPicker.branches,
                  onOpen: branchPicker.load,
                  onPick: (name) => {
                    if (branchLocked) {
                      toast.show(branchLocked);
                      return;
                    }
                    void branchPicker.pick(name);
                  },
                  onWorktreeFrom: (name) => void branchPicker.startWorktree(name),
                  locked: branchLocked,
                }}
                // A started chat stays in its folder, its branch shown in
                // Overview: only a new one picks either.
                folderFixed={!unstarted}
              />
            }
            end={<ProviderBalance providerId={llm.models.current?.providerId ?? null} running={agent.turn.status === "running"} />}
            index={workspace.path ? index : null}
            onSend={(text, images) => send(text, undefined, images)}
            imagesEnabled={imagesEnabled}
            onQueue={agent.queue}
            queued={agent.queued}
            onUnqueue={agent.unqueue}
            focus={composerFocus}
            // Not behind a page: the box is hidden there and the key would dictate into nothing to see.
            dictate={features.isOn("voice-input") && !onPage}
            layoutCorrection={features.isOn("layout-correction") && !onPage}
            switchLayoutAfterFix={features.layoutSwitch}
            onLayoutCorrectionError={toast.show}
            // The box is this chat's: what was typed in another one waits there.
            draftKey={chatDraftKey("agent", agent.chatId)}
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
            onCommandsOpen={() => {
              commandFiles.reload();
              serverPrompts.start();
            }}
            pendingCommands={serverPrompts.starting ? "Loading MCP prompts…" : undefined}
          />
          {bottomPane && (
            <PanelResizeHandle
              axis="y"
              invert
              ariaLabel="Resize the terminal"
              onResize={panels.resizeBottomBy}
              onResizeEnd={(size) => panels.endResize("bottom", size)}
            />
          )}
          {bottomPane}
          {agentShown && agentShown.chatId === agent.chatId && (
            <AgentSheet chatId={agent.chatId} run={agentShown.run} onClose={closeAgent} onOpenFile={workspace.path ? openFileLink : undefined} />
          )}
          {planSheet && <PlanSheet plan={panes.plan} onClose={hideAside} />}
        </>
      ) : (
        <PlainChat
          chat={plain}
          dictate={features.isOn("voice-input") && !onPage}
          layoutCorrection={features.isOn("layout-correction") && !onPage}
          switchLayoutAfterFix={features.layoutSwitch}
          onLayoutCorrectionError={toast.show}
          focus={composerFocus}
          models={llm.models}
          imagesEnabled={imagesEnabled}
          onModel={(choice) => llm.pickModel(choice.providerId, choice.model)}
          onEffort={llm.pickEffort}
          onLoadModels={llm.loadModels}
          kube={kube}
          onSetUpKube={() => openSettings("kubernetes")}
          onCompact={async () => {
            const folded = await plain.compact();
            // The reason comes back from the call: the hook's error is a render behind.
            if (folded !== true) toast.show(typeof folded === "string" ? folded : "Nothing worth folding away yet");
          }}
          onRewind={rewinding.ask}
          pages={
            plain.role === "prototype"
              ? {
                  available: prototypePreview.available,
                  open: previewShown,
                  onToggle: prototypePreview.toggle,
                  onOpenVersion: prototypePreview.show,
                  fix: prototypePreview.fix,
                  pointed: prototypePreview.pointed,
                  onUnpoint: () => prototypePreview.point(null),
                  onShowAssets: prototypePreview.showAssets,
                }
              : undefined
          }
        />
      )}
    </main>
  );
  const fileViewer = agentMode && viewer.active ? (
    <FileViewer
      key="viewer"
      files={viewer.files}
      active={viewer.active}
      preview={viewer.preview}
      workspace={workspace.path}
      onActivate={viewer.open}
      onPin={viewer.pin}
      onClose={viewer.close}
      onCloseAll={viewer.closeAll}
      wrap={wrapLines}
      compact={ide}
      edits={viewer.edits}
      reveal={viewer.reveal}
    />
  ) : previewShown && plain.chatId !== null ? (
    <PrototypePreview
      key="viewer"
      preview={prototypePreview}
      chatId={plain.chatId}
      onNotify={toast.show}
      onRestore={
        plain.busy
          ? undefined
          : async (version) => {
              if (!(await plain.restore(version))) return;
              // The folder as it now is — the version brought back.
              prototypePreview.show(null);
              prototypePreview.refresh();
            }
      }
    />
  ) : null;
  const topPane = (
    <AsidePanel key="top" tab={tab} dock="right" ctx={{ ...panes, active: !topHidden }} onClose={hideAside} />
  );
  const renderPage = (one: PageDef) => (
    <one.Component
      openPane={openTab}
      item={one === onPage ? pageItem : undefined}
      openPage={openPage}
      openChat={(id) => {
        leavePage();
        setMode("chat");
        void plain.open(id).then((opened) => opened || toast.show(STAY));
      }}
      removeChat={async (id) => {
        const removed = await plain.remove(id);
        if (removed) dropDraft("chat", id);
        return removed;
      }}
      chats={plain.chats}
    />
  );

  return (
    <div
      className={`window${nativeFrame ? " native-frame" : ""}${windowsFrame ? " windows-frame" : ""}${rail ? " collapsed" : ""}${dockShown ? "" : " aside-hidden"}${ide ? " ide" : ""}${onPage ? " on-page" : ""}${previewShown && prototypePreview.expanded ? " preview-expanded" : ""}`}
      style={
        {
          "--sidebar-width": `${panels.widths.sidebar}px`,
          "--aside-width": `${panels.widths.aside}px`,
          "--bottom-height": `${panels.widths.bottom}px`,
          "--viewer-width": `${panels.widths.viewer}px`,
          "--chat-width": `${panels.widths.chat}px`,
        } as React.CSSProperties
      }
    >
      <Titlebar
        slot={setHeadSlot}
        leading={
          // The IDE layout has no sidebar to show.
          !ide && (
            <>
              <button
                className="iconbtn"
                type="button"
                title={`${rail ? "Show the sidebar" : "Hide the sidebar"} (${shortcutText("sidebar")})`}
                aria-pressed={!rail}
                onClick={toggleSidebar}
              >
                <PanelLeft size={15} />
              </button>
              <button className="iconbtn" type="button" title={`New chat (${shortcutText("newChat")})`} aria-label="New chat" onClick={newChat}>
                <SquarePen size={15} />
              </button>
            </>
          )
        }
      >
        {/* The open page's name, or the open chat's where the chat is on screen; the IDE layout's header names it already. */}
        {onPage?.label ||
          (!ide && (agentMode ? chatName(history.chats, agent.chatId, agent.turn) : chatName(plain.chats, plain.chatId, plain.turn))) ||
          "Kibo Agent"}
      </Titlebar>

      <div className="body">
        <Sidebar
          mode={mode}
          onMode={(next) => {
            leavePage();
            setMode(next);
          }}
          pages={pages}
          page={onPage?.id ?? null}
          onPage={openPage}
          sections={
            agentMode
              ? workspace.folders.map((path) => ({
                  path,
                  worktreeOf: workspace.worktrees[path] ?? null,
                  chats: history.byFolder[path] ?? [],
                }))
              : [{ path: null, chats: plain.chats }]
          }
          groups={chatGroups.groups}
          activeFolder={agentMode ? workspace.path : null}
          onNewChatIn={agentMode ? newChatIn : undefined}
          onCloseFolder={agentMode ? closeFolder : undefined}
          onMoveFolder={agentMode ? workspace.move : undefined}
          activeChat={agentMode ? agent.chatId : plain.chatId}
          chatActivity={agentMode ? agent.chatActivity : plain.chatActivity}
          onSelectChat={selectChat}
          {...sidebarActions}
          onOpenSettings={() => openSettings()}
          onOnboardingAction={openTab}
          themeSide={sideOf(theme.choice)}
          onToggleTheme={theme.toggle}
        />

        <PanelResizeHandle
          ariaLabel="Resize the chat list"
          onResize={panels.resizeSidebarBy}
          onResizeEnd={(size) => panels.endResize("sidebar", size)}
        />

        <Columns
          ide={ide}
          chat={chat}
          pageShown={onPage !== null}
          page={
            onPage || keptPages.length > 0 ? (
              <Fragment key="page">
                {keptPages.map((one) => (
                  <div key={one.id} className="page-kept" hidden={one !== onPage}>
                    {renderPage(one)}
                  </div>
                ))}
                {onPage && !keptPages.includes(onPage) && renderPage(onPage)}
              </Fragment>
            ) : null
          }
          viewer={fileViewer}
          top={topPane}
          topShown={dockShown}
          panels={panels}
        />
      </div>

      <FolderSwitchDialog
        blocked={folderSwitch.blocked}
        onStopAgent={(chats) => void agent.cancelAll(chats)}
        onClose={folderSwitch.close}
      />
      <FolderTrustDialog
        trust={trust.trust}
        open={trust.asking}
        error={trust.error}
        onDecide={(trusted) => void trust.decide(trusted)}
        onClose={trust.close}
      />
      <RewindDialog
        asked={rewinding.asked}
        summarizing={rewinding.summarizing}
        onConfirm={(summarize) => void rewinding.confirm(summarize)}
        onClose={rewinding.close}
      />
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

      <SettingsDialog
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        section={settingsAt}
        kube={kube}
        skills={{ view: skillSources.view, error: skillSources.error, onToggle: skillSources.setSourceEnabled }}
        provider={{
          settings: llm.settings,
          busy: llm.busy,
          error: llm.error,
          onSave: llm.save,
          onRemove: llm.remove,
          onSelect: llm.select,
          onKeyStore: llm.keyStore,
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
        turnLimits={llm.settings?.turnLimits ?? DEFAULT_TURN_LIMITS}
        onTurnLimits={llm.turnLimits}
        theme={theme.choice}
        onThemeMode={theme.setMode}
        onThemePalette={theme.setPalette}
        fontSize={fontSize.size}
        onFontSize={fontSize.setSize}
        wrapLines={wrapLines}
        onWrapLines={setWrapLines}
        alerts={alerts}
        onAlerts={setAlerts}
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

      <McpConfigDialog dialog={mcpDialog} view={mcp.view} error={mcp.error} onSave={mcp.save} onClose={closeMcp} onEditJson={editMcpFile} />

      <HooksConfigDialog dialog={hooksDialog} view={hooks.view} error={hooks.error} onSave={hooks.save} onClose={closeHooks} onEditJson={editHooksFile} />

      <Toast message={toast.message} />
    </div>
  );
}
