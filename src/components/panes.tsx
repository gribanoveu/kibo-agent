import { lazy, Suspense, type ReactNode } from "react";
import {
  BookText,
  ClipboardList,
  FolderClosed,
  Plug,
  Sparkles,
  SquareTerminal,
  FileDiff,
  Webhook,
} from "lucide-react";
import { ChangesPanel } from "./ChangesPanel";
import { FilesPanel } from "./FilesPanel";
import { ItemList } from "./ItemList";
import { PlanPanel } from "./PlanPanel";
import { useRules } from "../hooks/useRules";
import { useSkills } from "../hooks/useSkills";
import type { FileTarget, HooksView, McpServerItem, McpServerState, McpToolInfo, McpView, RuleListItem, SkillListItem, SkillsView, Task } from "../lib/chat";
import type { AsideTab, PanelItem } from "../types";
import type { Block } from "../lib/chatTurnReducer";
import type { AgentFocus } from "../lib/describeTool";

/**
 * What every pane is drawn from. A pane that only it needs reads its own data
 * (skills, rules, processes); what something else in the window also reads —
 * Settings shows the MCP servers and hooks, the chat owns the plan — is read
 * once in `App` and handed in here.
 */
export type PaneContext = {
  /** On screen right now. A pane asks the backend only while it is. */
  active: boolean;
  workspace: string | null;
  /** The window is in the docs layout: the Files pane shows the tree. */
  docs: boolean;
  onNotify: (msg: string) => void;
  /** The commit message being written in Changes. */
  commitDraft: { message: string; onMessage: (message: string) => void };
  /** The background process a chat row asked to see; a new object each ask. */
  processFocus: { id: number } | null;
  /** The helper run a chat row asked to see; a new object each ask. */
  agentFocus: AgentFocus | null;
  /** A command an answer asked to put in a shell; a new object each ask. */
  terminalPaste: { command: string } | null;
  /** The Terminal took it: not again when the pane is drawn anew. */
  onTerminalPasted: () => void;
  /** Puts text into the message being written — a selection from the terminal. */
  onAddToChat: (text: string) => void;
  /** The open chat's transcript, for the files its calls touched. */
  chatBlocks: Block[];
  /** The file the viewer beside the chat shows, marked where it is listed. */
  openFile: FileTarget | null;
  /** Shows a file in the viewer beside the chat: a single click previews it, `pin` keeps its tab. */
  onOpenFile: (target: FileTarget, pin?: boolean) => void;
  mcp: McpListProps;
  hooks: HooksListProps;
  plan: {
    plan: string | null;
    checklist: Task[];
    onEdit: (plan: string) => void;
    onImplement?: () => void;
    locked: boolean;
  };
};

/** Where a pane opens: the column right of the chat, or the strip under it. */
export type Dock = "right" | "bottom";

export type PaneDef = {
  id: AsideTab;
  label: string;
  icon: typeof FileDiff;
  dock: Dock;
  Component: (ctx: PaneContext) => ReactNode;
};

// ─── MCP ───────────────────────────────────────────────────────────────────

export type McpListProps = {
  view: McpView | null;
  error: string | null;
  onToggle: (name: string, enabled: boolean) => void;
  /** A tool offered to the model or hidden from it. */
  onToggleTool: (server: string, tool: string, shown: boolean) => void;
  /** Opening a server's row starts it, so the row can list its tools. */
  onOpen: (name: string) => void;
  /** A new server, one server, or the whole file. */
  onAdd: () => void;
  onEditServer: (name: string) => void;
  onRemoveServer: (name: string) => void;
  onEditFile: () => void;
};

/**
 * A server that cannot start says why and has no switch, like a broken skill.
 * As with skills, the ones on come first and the ones off after them, grey.
 */
function mcpItems(view: McpView | null): PanelItem[] {
  const off = (s: McpServerItem) => !s.error && !s.enabled;
  const servers = [...(view?.servers ?? [])].sort((a, b) => Number(off(a)) - Number(off(b)));
  return servers.map((server) =>
    server.error
      ? {
          id: server.name,
          badge: "!",
          kind: "mcp",
          title: server.name,
          status: { label: "won't start", tone: "warn" },
          desc: server.error,
          source: server.command || undefined,
        }
      : {
          id: server.name,
          badge: server.name.slice(0, 2).toUpperCase(),
          kind: "mcp",
          title: server.name,
          desc: server.command,
          enabled: server.enabled,
          dim: !server.enabled,
          // The process's own trouble, when it has one, says more than the entry's.
          note: server.warning ?? undefined,
          ...(server.enabled ? mcpState(server.state, server.warning) : {}),
        },
  );
}

/** "12 tools", and how many of them the model does not see. */
function toolCount(tools: McpToolInfo[]): string {
  const hidden = tools.filter((t) => t.exposure === "hidden").length;
  const all = `${tools.length} ${tools.length === 1 ? "tool" : "tools"}`;
  return hidden ? `${all} · ${hidden} hidden` : all;
}

/** What the process is doing, for a server that is switched on. */
function mcpState(state: McpServerState, warning: string | null): Partial<PanelItem> {
  switch (state.state) {
    case "notStarted":
      return { meta: "Open this row to start it and see its tools" };
    case "starting":
      return { status: { label: "starting", tone: "off" }, meta: "Asking it for its tools" };
    case "running":
      return {
        status: { label: toolCount(state.tools), tone: "ok" },
        rows: state.tools.map((tool) => ({
          id: tool.name,
          // The server's word about its own tool, said as such by where it stands: beside the name.
          name:
            tool.name +
            (tool.destructive ? " · destructive" : tool.readOnly ? " · read-only" : "") +
            (tool.exposure === "deferred" ? " · via search" : ""),
          desc: tool.title ? `${tool.title} — ${tool.description}` : tool.description,
          enabled: tool.exposure !== "hidden",
        })),
        // What the server tells the model is said on the user's behalf, so the user can read it.
        ...(state.instructions
          ? { note: [warning, `It tells the model: ${state.instructions}`].filter(Boolean).join("\n\n") }
          : {}),
      };
    case "exited":
      return { status: { label: "exited", tone: "warn" }, meta: "Restarts with the next call", note: state.error };
    case "failed":
      return { status: { label: "failed", tone: "warn" }, meta: "Switch off and on to try again", note: state.error };
  }
}

export function McpList({ view, error, onToggle, onToggleTool, onOpen, onAdd, onEditServer, onRemoveServer, onEditFile }: McpListProps) {
  const items = mcpItems(view);
  return (
    <>
      <ItemList
        label="Servers"
        count={`${items.filter((s) => s.enabled).length}/${items.length}`}
        items={items}
        emptyLabel="No MCP servers configured."
        addLabel="Add MCP server"
        onAdd={onAdd}
        onToggle={onToggle}
        onToggleRow={onToggleTool}
        onOpen={onOpen}
        onEdit={onEditServer}
        onRemove={onRemoveServer}
        onEditFile={onEditFile}
      />
      {error && <div className="empty">{error}</div>}
    </>
  );
}

// ─── Hooks ─────────────────────────────────────────────────────────────────

export type HooksListProps = {
  view: HooksView | null;
  error: string | null;
  /** A new hook, one hook by its row, or the whole file. */
  onAdd: () => void;
  onEditHook: (index: number) => void;
  onRemoveHook: (index: number) => void;
  onEditFile: () => void;
};

const HOOK_BADGES: Record<string, string> = { PreToolUse: "PRE", PostToolUse: "PST", Stop: "STP" };

/** One row per command. What the tool name must match is part of the title, since it decides when it runs at all. */
function hookItems(view: HooksView | null): PanelItem[] {
  return (view?.hooks ?? []).map((hook, i) => ({
    id: `${i}`,
    badge: HOOK_BADGES[hook.event] ?? "?",
    kind: "hook",
    title: hook.event === "Stop" ? "Stop" : `${hook.event} · ${hook.matcher.trim() || "every tool"}`,
    desc: hook.command,
    ...(hook.problem
      ? { status: { label: "won't run", tone: "warn" as const }, meta: hook.problem }
      : { meta: `Stopped after ${hook.timeoutSecs} s` }),
  }));
}

export function HooksList({ view, error, onAdd, onEditHook, onRemoveHook, onEditFile }: HooksListProps) {
  const items = hookItems(view);
  return (
    <>
      <ItemList
        label="Hooks"
        count={`${items.filter((h) => !h.status).length}/${items.length}`}
        items={items}
        emptyLabel="No hooks. A hook runs a command before a tool call, after one, or when the agent finishes."
        addLabel="Add a hook"
        onAdd={onAdd}
        onEdit={(id) => onEditHook(Number(id))}
        onRemove={(id) => onRemoveHook(Number(id))}
        onEditFile={onEditFile}
      />
      {error && <div className="empty">{error}</div>}
    </>
  );
}

// ─── Skills ────────────────────────────────────────────────────────────────

type SkillsListProps = {
  view: SkillsView | null;
  error: string | null;
  /** By name: a skill switched off is off in every folder and repository. */
  onToggle: (name: string, enabled: boolean) => void;
};

/**
 * `~/.agents/skills` for a skill in it: which of the user's folders a row is
 * from. Either separator: on Windows the path comes with backslashes.
 */
export function folderOf(path: string): string {
  const dir = path.replace(/[\\/][^\\/]*$/, "");
  const agent = /[\\/](\.kibo|\.agents|\.claude)[\\/]skills$/.exec(dir);
  return agent ? `~/${agent[1]}/skills` : dir;
}

/** A broken skill is shown by its folder with the reason, and has no switch: it never reaches the model. */
function skillItems(found: SkillListItem[], project: boolean, all: SkillListItem[]): PanelItem[] {
  // The ones on first, the ones off after them and grey; a broken one has no
  // switch and stays where it was found. The sort is stable, so each group
  // keeps the order a name is looked up in.
  const off = (s: SkillListItem) => !s.error && !s.enabled;
  const skills = [...found].sort((a, b) => Number(off(a)) - Number(off(b)));
  // Where the skill used instead of a hidden one is, in a word or two: the
  // full path is in the expanded row, and a card has no room for it.
  const usedFrom = (path: string) =>
    all.find((s) => s.path === path)?.source === "project" ? "this repository" : folderOf(path);
  return skills.map((skill) =>
    skill.error
      ? {
          id: skill.path,
          badge: "!",
          kind: "skill",
          title: skill.name,
          status: { label: "invalid", tone: "warn" },
          desc: skill.error,
          source: skill.path,
        }
      : {
          id: skill.path,
          badge: skill.name.slice(0, 2).toUpperCase(),
          kind: "skill",
          title: skill.name,
          // On, but another skill of this name comes first and is the one used.
          ...(skill.shadowedBy
            ? { status: { label: "hidden", tone: "off" as const }, meta: `Used instead: ${usedFrom(skill.shadowedBy)}` }
            : {}),
          ...(project ? {} : { tags: [folderOf(skill.path)] }),
          desc: skill.description,
          enabled: skill.enabled,
          dim: !skill.enabled,
          note: skill.description,
          source: skill.path,
        },
  );
}

const count = (items: PanelItem[]) => `${items.filter((s) => s.enabled).length}/${items.length}`;

/**
 * The repository's skills first — they are the ones used when a name is in
 * both — then the user's own. The repository's section is there only when
 * the open folder has any.
 */
export function SkillsList({ view, error, onToggle }: SkillsListProps) {
  const all = view?.skills ?? [];
  const project = skillItems(all.filter((s) => s.source === "project"), true, all);
  const mine = skillItems(all.filter((s) => s.source === "user"), false, all);
  // Rows are told apart by folder; the switch goes by the name in it.
  const byPath = new Map(all.map((s) => [s.path, s.name]));
  const toggle = (path: string, enabled: boolean) => onToggle(byPath.get(path) ?? path, enabled);
  return (
    <>
      {project.length > 0 && (
        <ItemList label="This repository" count={count(project)} items={project} emptyLabel="" onToggle={toggle} />
      )}
      <ItemList
        label={project.length > 0 ? "Yours" : "Skills"}
        count={count(mine)}
        items={mine}
        emptyLabel={
          project.length > 0
            ? `None of your own. A skill is a folder with a SKILL.md in ${view?.dir || "the app directory"}, ~/.agents/skills or ~/.claude/skills.`
            : `No skills yet. A skill is a folder with a SKILL.md — in ${view?.dir || "the app directory"}, ~/.agents/skills or ~/.claude/skills for every repository, or in the repository's .claude/skills or .agents/skills.`
        }
        onToggle={toggle}
      />
      {error && <div className="empty">{error}</div>}
    </>
  );
}

function SkillsPane({ active, workspace }: PaneContext) {
  const skills = useSkills(active, workspace);
  return <SkillsList view={skills.view} error={skills.error} onToggle={skills.setEnabled} />;
}

// ─── Rules ─────────────────────────────────────────────────────────────────

type RulesListProps = {
  rules: RuleListItem[];
  error: string | null;
  onToggle: (path: string, enabled: boolean) => void;
};

/** Shown whole on expand: what the model is told is worth reading. The switch is keyed by path. */
function ruleItems(rules: RuleListItem[]): PanelItem[] {
  return rules.map((rule) =>
    rule.error
      ? {
          id: rule.path,
          badge: "!",
          kind: "rule",
          title: rule.name,
          status: { label: "not sent", tone: "warn" },
          desc: rule.error,
          source: rule.path,
        }
      : {
          id: rule.path,
          badge: rule.name.slice(0, 2).toUpperCase(),
          kind: "rule",
          title: rule.name,
          ...(rule.truncated ? { status: { label: "cut", tone: "warn" as const } } : {}),
          desc: `${rule.content.split("\n").length} lines${rule.truncated ? " sent, the rest cut" : ""}`,
          enabled: rule.enabled,
          note: rule.content,
          source: rule.path,
        },
  );
}

export function RulesList({ rules, error, onToggle }: RulesListProps) {
  const items = ruleItems(rules);
  return (
    <>
      <ItemList
        label="Project instructions"
        count={`${items.filter((r) => r.enabled).length}/${items.length}`}
        items={items}
        emptyLabel="No AGENTS.md or CLAUDE.md at the root of the open folder."
        onToggle={onToggle}
      />
      {error && <div className="empty">{error}</div>}
    </>
  );
}

function RulesPane({ active, workspace }: PaneContext) {
  const rules = useRules(active, workspace);
  return <RulesList rules={rules.rules} error={rules.error} onToggle={rules.setEnabled} />;
}

// ─── Files ─────────────────────────────────────────────────────────────────

// Filled by its own command wrapper once that command exists.
function FilesPane({ active, workspace, docs, chatBlocks, openFile, onOpenFile }: PaneContext) {
  return <FilesPanel active={active} blocks={chatBlocks} workspace={workspace} openFile={openFile} onOpenFile={onOpenFile} docs={docs} />;
}

// ─── The registry ──────────────────────────────────────────────────────────

// Every pane the window can show, in the order the chat header's "⋮" lists
// them. Adding one is an id in `ASIDE_TABS` and an entry here — nothing else
// names a pane. `dock` is where the menu opens it; each dock shows one pane. There is no tab strip: eight icons in a 300px column were
// unreadable, and only one of them is opened often.
// The terminal brings xterm, a quarter of the app's script, and is opened
// far less often than the app is: it is loaded the first time it is shown.
const TerminalPanel = lazy(() => import("./TerminalPanel").then((m) => ({ default: m.TerminalPanel })));

export const PANES: PaneDef[] = [
  { id: "changes", label: "Changes", icon: FileDiff, dock: "right", Component: ({ active, workspace, onNotify, commitDraft, openFile, onOpenFile }) => <ChangesPanel active={active} workspace={workspace} onNotify={onNotify} openFile={openFile} onOpenFile={onOpenFile} {...commitDraft} /> },
  { id: "plan", label: "Plan", icon: ClipboardList, dock: "right", Component: ({ plan }) => <PlanPanel {...plan} /> },
  { id: "files", label: "Files", icon: FolderClosed, dock: "right", Component: FilesPane },
  { id: "rules", label: "Rules", icon: BookText, dock: "right", Component: RulesPane },
  { id: "skills", label: "Skills", icon: Sparkles, dock: "right", Component: SkillsPane },
  { id: "mcp", label: "MCP", icon: Plug, dock: "right", Component: ({ mcp }) => <McpList {...mcp} /> },
  { id: "hooks", label: "Hooks", icon: Webhook, dock: "right", Component: ({ hooks }) => <HooksList {...hooks} /> },
  { id: "terminal", label: "Terminal", icon: SquareTerminal, dock: "bottom", Component: (ctx) => (
      <Suspense fallback={null}>
        <TerminalPanel {...ctx} />
      </Suspense>
    ),
  },
];
