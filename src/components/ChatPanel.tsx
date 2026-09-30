import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  ArrowUpRight,
  Brain,
  Loader2,
  ChevronRight,
  FileText,
  Folder,
  FileDiff,
  FolderTree,
  FoldVertical,
  GitBranch,
  ListTodo,
  MessageSquare,
  Pencil,
  Download,
  Search,
  ShieldAlert,
  ShipWheel,
  Terminal,
  TerminalSquare,
  Trash2,
  RotateCcw,
} from "lucide-react";
import { ChatEmptyState } from "./ChatEmptyState";
import { FindingCard } from "./ReviewCard";
import { findingOf } from "../lib/finding";
import { ChatMenu, type ChatMenuItem } from "./ChatMenu";
import { DiffView } from "./DiffView";
import { PANES } from "./panes";
import { shortcutText } from "../lib/shortcuts";
import type { AsideTab } from "../types";
import { CopyAction } from "./CopyAction";
import { Markdown } from "./Markdown";
import { describeActive, describeRun, describeTool, type AgentFocus } from "../lib/describeTool";
import { useFollowBottom } from "../hooks/useFollowBottom";
import { useSteadyValue } from "../hooks/useSteadyValue";
import type { Block, TurnState } from "../lib/chatTurnReducer";
import {
  previewCalls,
  processStatus,
  type ProcessInfo,
  type ToolCallDecision,
  type PendingToolCall,
  type FileDiffStats,
  type ToolPreview,
} from "../lib/chat";
import "./ChatPanel.css";

const TOOL_ICON: Record<string, typeof FileText> = {
  Read: FileText,
  Grep: Search,
  List: FolderTree,
  Write: Pencil,
  Edit: Pencil,
  Delete: Trash2,
  Mkdir: FolderTree,
  Move: FolderTree,
  Todo: ListTodo,
  Bash: TerminalSquare,
  Status: GitBranch,
  Diff: GitBranch,
  Blame: GitBranch,
};


/** Opens a background process in the Terminal tab. */
type OpenProcess = (id: number) => void;
type OpenAgent = (agent: AgentFocus) => void;

/**
 * A background process that ended, as the model was told of it: how it
 * ended, what it was, where it ran. Opens it in the Terminal tab, where its
 * output is.
 */
function ProcessEndedCard({ process, onOpen }: { process: ProcessInfo; onOpen?: OpenProcess }) {
  const state = process.state;
  const tone = state.state === "exited" && state.code !== 0 ? "failed" : state.state === "stopped" ? "stopped" : "ended";
  return (
    <button
      type="button"
      className={`process-ended ${tone}`}
      title={onOpen ? "Show in Terminal" : undefined}
      disabled={!onOpen}
      onClick={() => onOpen?.(process.id)}
    >
      <span className="process-ended-ico" aria-hidden>
        <TerminalSquare size={14} />
      </span>
      <span className="process-ended-body">
        <code className="process-ended-command">{process.command}</code>
        <span className="process-ended-meta">
          <span className="process-ended-label">
            Background process #{process.id} {state.state === "stopped" ? "stopped" : "ended"}
          </span>
          <span className="process-ended-cwd">
            <Folder size={11} aria-hidden />
            {process.cwd === "." ? "project root" : process.cwd}
          </span>
        </span>
      </span>
      <span className="process-ended-state">{processStatus(state)}</span>
      {onOpen && <ArrowUpRight className="process-ended-open" size={13} aria-hidden />}
    </button>
  );
}

/**
 * A pass folding older history into a summary. It takes seconds — a request of
 * its own — so it is shown while it runs rather than only once it is done.
 */
function CompactionCard({ block }: { block: Extract<Block, { kind: "compaction" }> }) {
  const n = block.folded ?? 0;
  const [title, detail] =
    block.status === "running"
      ? ["Compacting history…", "Summarizing older messages so the conversation fits the context window"]
      : block.status === "done"
        ? [
            "History compacted",
            `${n} message${n === 1 ? "" : "s"} folded into a summary — the model sees the summary, this transcript keeps them all`,
          ]
        : ["History not compacted", "No summary was made — the conversation was left as it was"];
  return (
    <div className={`compaction-card ${block.status}`} role="status">
      <span className="compaction-ico" aria-hidden>
        {block.status === "running" ? <Loader2 className="tool-run-spin" size={14} /> : <FoldVertical size={14} />}
      </span>
      <span className="compaction-body">
        <span className="compaction-title">{title}</span>
        <span className="compaction-detail">{detail}</span>
      </span>
    </div>
  );
}

function ToolRow({
  block,
  onOpenProcess,
  onOpenAgent,
}: {
  block: Extract<Block, { kind: "tool" }>;
  onOpenProcess?: OpenProcess;
  onOpenAgent?: OpenAgent;
}) {
  const [open, setOpen] = useState(false);
  const shown = describeTool(block);
  // A cluster call wears its role's sign, whatever its label.
  const Icon = block.name.startsWith("kube") ? ShipWheel : (TOOL_ICON[shown.name] ?? Terminal);
  // A background start has nothing to unfold here: its output is the
  // Terminal tab's, and the row goes there.
  const process = shown.process !== undefined && onOpenProcess ? shown.process : undefined;

  return (
    <div className={`tool-item${open ? " open" : ""} ${block.status}`}>
      <button
        className="tool"
        type="button"
        title={process !== undefined ? "Show in Terminal" : undefined}
        onClick={() => (process !== undefined ? onOpenProcess?.(process) : setOpen((v) => !v))}
        disabled={!shown.detail && process === undefined}
      >
        <span className="ico">
          <Icon size={13} />
        </span>
        <span className="name">{shown.name}</span>
        <span className="arg">{shown.arg}</span>
        {shown.meta && <span className="meta">{shown.meta}</span>}
        {process !== undefined ? (
          <ArrowUpRight className="chev" size={12} />
        ) : (
          shown.detail && <ChevronRight className="chev" size={12} />
        )}
      </button>
      {/* Unfolded rather than instead of it, as a process row does: the
          answer stays readable here after the Agents tab has forgotten the run. */}
      {open && shown.agent && onOpenAgent && (
        <button type="button" className="tool-open-agent" onClick={() => onOpenAgent(shown.agent!)}>
          Show in Agents
          <ArrowUpRight size={12} aria-hidden />
        </button>
      )}
      {open &&
        shown.detail &&
        (shown.diff ? (
          <div className="tool-detail-diff">
            <DiffView unified={shown.detail} />
          </div>
        ) : (
          <pre className="tool-detail">{shown.detail}</pre>
        ))}
    </div>
  );
}

function ApprovalCard({
  block,
  onDecide,
  preview = previewCalls,
}: {
  block: Extract<Block, { kind: "approval" }>;
  onDecide: (decisions: ToolCallDecision[], always: string[]) => void;
  preview?: PreviewCalls;
}) {
  const [reason, setReason] = useState("");
  // What each call would do. Approving a write means approving its contents,
  // and the arguments alone do not show them.
  const [previews, setPreviews] = useState<ToolPreview[]>([]);
  const asked = block.calls.filter((call) => call.requiresConfirmation);
  // Bundled into the round but not in question — six todo updates are one
  // line, not six lines of "update" between the diff and the buttons.
  const passive = countedNames(
    block.calls.filter((call) => !call.requiresConfirmation).map((call) => describeTool({ ...emptyTool, ...call }).name),
  );

  useEffect(() => {
    let live = true;
    preview(block.calls)
      // `?? []`: the card is worth drawing even if the previews are not.
      .then((next) => live && setPreviews(next ?? []))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [block.calls, preview]);
  const answer = (approved: boolean, always: string[] = []) =>
    onDecide(
      asked.map((call) => ({
        id: call.id,
        approved,
        reason: approved || !reason.trim() ? null : reason.trim(),
      })),
      always,
    );

  return (
    <div className="approval-card">
      <div className="approval-label">
        Approval required · {asked.map((call) => describeTool({ ...emptyTool, ...call }).name).join(", ")}
      </div>
      {block.calls.map((call, index) =>
        call.requiresConfirmation ? (
          <div key={call.id}>
            {/* A diff names its file in its own header. */}
            {previews[index]?.kind !== "diff" && previews[index]?.kind !== "change" && (
              <div className="approval-cmd">{describeTool({ ...emptyTool, ...call }).arg}</div>
            )}
            {call.reason && (
              <div className="approval-why" title="Asked even when this tool is always allowed">
                <ShieldAlert size={13} aria-hidden />
                <span>Always asks: {call.reason}</span>
              </div>
            )}
            <Preview preview={previews[index]} />
          </div>
        ) : null,
      )}
      {passive.length > 0 && (
        <div className="approval-cmd passive">Also runs, no approval needed: {passive}</div>
      )}
      <input
        className="approval-reason"
        type="text"
        placeholder="Why not? (optional — the agent is told)"
        value={reason}
        onChange={(e) => setReason(e.target.value)}
      />
      <div className="approval-actions">
        <button className="btn btn-primary" type="button" onClick={() => answer(true)}>
          Allow
        </button>
        <button
          className="btn btn-ghost"
          type="button"
          onClick={() => answer(true, asked.map((call) => call.name))}
        >
          Always
        </button>
        <button className="btn btn-ghost" type="button" onClick={() => answer(false)}>
          Deny
        </button>
      </div>
    </div>
  );
}

/** What the call would do, once the backend has worked it out. */
export function Preview({ preview }: { preview?: ToolPreview }) {
  if (!preview || preview.kind === "nothing") return null;

  if (preview.kind === "failed") {
    // Worth as much as a successful preview: an edit whose anchor no longer
    // matches is better refused before agreeing to it than after.
    return <p className="approval-preview failed">This would not succeed: {preview.reason}</p>;
  }

  if (preview.kind === "removes") {
    return (
      <p className="approval-preview">
        Removes {preview.files} {preview.files === 1 ? "file" : "files"} under {preview.path}
      </p>
    );
  }

  if (preview.kind === "change") {
    // Where first: the cluster is what a wrong approval costs most.
    return (
      <div className="approval-change">
        <div className={`approval-change-place${preview.production ? " production" : ""}`}>
          {preview.production && "PRODUCTION · "}
          {preview.place}
        </div>
        <div className="approval-cmd">{preview.summary}</div>
        {preview.notes.map((note) => (
          <p className="approval-preview" key={note}>
            {note}
          </p>
        ))}
        {preview.diffs?.map((object) => <DiffPreview key={object.title} title={object.title} diff={object.diff} />)}
      </div>
    );
  }

  if (preview.kind === "command") {
    return <p className="approval-preview">Runs in {preview.cwd}</p>;
  }

  return <DiffPreview title={preview.path} diff={preview.diff} />;
}

/** What becomes of one thing — a file, a cluster's object — under its name. */
function DiffPreview({ title, diff }: { title: string; diff: FileDiffStats }) {
  return (
    <div className="diff-preview-wrap">
      <div className="diff-preview-head">
        <span className="diff-preview-name">{title}</span>
        <span className="meta mono">
          <span className="add">+{diff.linesAdded}</span>{" "}
          <span className={diff.linesRemoved ? "del" : "zero"}>-{diff.linesRemoved}</span>
        </span>
      </div>
      <DiffView unified={diff.unifiedDiff} />
      {diff.truncated && <p className="approval-preview">…the rest is not shown</p>}
    </div>
  );
}

/** `["Todo", "Todo", "Read"]` → `"Todo ×2, Read"`, first-seen order. */
function countedNames(names: string[]): string {
  const counts = new Map<string, number>();
  for (const name of names) counts.set(name, (counts.get(name) ?? 0) + 1);
  return [...counts].map(([name, n]) => (n > 1 ? `${name} ×${n}` : name)).join(", ");
}

/** A pending call has no result yet, and `describeTool` reads the same shape either way. */
const emptyTool = {
  kind: "tool" as const,
  round: 0,
  status: "running" as const,
  output: "",
};

/**
 * The flat block stream, grouped the way the prototype draws it: a user bubble
 * opens a turn, and everything until the next one belongs to the agent.
 */
type Group = { role: "user" | "agent" | "notice"; blocks: Block[] };

function group(blocks: Block[]): Group[] {
  const groups: Group[] = [];
  for (const block of blocks) {
    // A notice is nobody's turn — it is the app saying what it did — so it
    // stands alone rather than appearing under "Agent" as something said. A
    // process that ended is news of the same kind.
    const role =
      block.kind === "notice" || block.kind === "processEnded" || block.kind === "compaction"
        ? "notice"
        : block.kind === "user" || block.kind === "steer"
          ? "user"
          : "agent";
    const last = groups[groups.length - 1];
    if (!last || last.role !== role || block.kind === "user" || role === "notice") {
      groups.push({ role, blocks: [block] });
    } else {
      last.blocks.push(block);
    }
  }
  return groups;
}

/** 12s, 1m 23s, 1h 5m — the way Claude Code says how long it worked. */
export function formatDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

/**
 * "· 2 processes running" beside the clock: background processes outlive the
 * turn, and without this a server left running is out of sight. A click
 * opens the newest in the Terminal tab.
 */
function RunningProcesses({ ids, onOpen }: { ids: number[]; onOpen?: (id: number) => void }) {
  if (ids.length === 0) return null;
  const label = `${ids.length} ${ids.length === 1 ? "process" : "processes"} running`;
  return (
    <>
      <span className="turn-clock-sep">·</span>
      {onOpen ? (
        <button type="button" className="turn-processes" title="Show in the Terminal tab" onClick={() => onOpen(ids[0])}>
          {label}
        </button>
      ) : (
        <span className="turn-processes">{label}</span>
      )}
    </>
  );
}

/** The time the agent has spent on the turn under way, ticking. */
function WorkingClock({ since, before, children }: { since: number; before: number; children?: ReactNode }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);
  return (
    <div className="turn-clock live" role="timer">
      <PawLoader />
      <span className="turn-clock-text">Working…</span>{" "}
      <span className="turn-clock-time">{formatDuration(before + Math.max(0, now - since))}</span>
      {children}
    </div>
  );
}

/** A paw stepping in place while the agent works; still under reduced motion. */
function PawLoader() {
  const box = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    let anim: { destroy(): void } | undefined;
    let gone = false;
    // Loaded on first use: the player is only needed while a turn runs, and
    // it touches <canvas> on import, which the test DOM does not have.
    Promise.all([import("lottie-web/build/player/lottie_light"), import("../assets/paw-loader.json")])
      .then(([{ default: lottie }, { default: animationData }]) => {
        if (gone || !box.current) return;
        anim = lottie.loadAnimation({
          container: box.current,
          renderer: "svg",
          loop: true,
          autoplay: !window.matchMedia("(prefers-reduced-motion: reduce)").matches,
          animationData,
          // The paw fills only the middle of its 1080×1080 canvas.
          rendererSettings: { viewBoxSize: "220 307 640 640" },
        });
      })
      .catch(() => {}); // no paw is fine: the words still say it is working
    return () => {
      gone = true;
      anim?.destroy();
    };
  }, []);
  return <span ref={box} className="turn-clock-paw" aria-hidden="true" />;
}

/** The message that started the turn a group belongs to. */
function turnMessage(groups: Group[], index: number) {
  for (let i = index; i >= 0; i--) {
    const found = groups[i].blocks.find((b) => b.kind === "user");
    if (found?.kind === "user") return found;
  }
  return undefined;
}

/**
 * "Worked for 1m 23s" under the last group of a finished turn's answer. The
 * turn under way shows its ticking clock at the end of the thread instead.
 */
function workedFooter(groups: Group[], index: number, turn: TurnState, extra?: ReactNode) {
  const next = groups[index + 1];
  if (groups[index].role !== "agent" || (next && next.role !== "user")) return null;
  const live = !next && (turn.status === "running" || turn.status === "awaitingApproval");
  const worked = turnMessage(groups, index)?.workedMs;
  if (live || !worked) return null;
  return (
    <div className="turn-clock">
      <span>Worked for {formatDuration(worked)}</span>
      {extra}
    </div>
  );
}

type Tool = Extract<Block, { kind: "tool" }>;
type Run = { kind: "run"; id: string; blocks: Block[] };

/**
 * An agent's calls and thinking between two things it said, folded into one
 * line — the answer is what the reader came for, the work behind it is a click
 * away. Approvals stay out: they wait for an answer and must be seen. So do a
 * review's findings: they are what the work was for.
 */
function fold(blocks: Block[]): (Block | Run)[] {
  const out: (Block | Run)[] = [];
  for (const block of blocks) {
    const last = out[out.length - 1];
    if ((block.kind !== "tool" && block.kind !== "reasoning") || findingOf(block)) {
      out.push(block);
    } else if (last?.kind === "run") {
      last.blocks.push(block);
    } else {
      out.push({ kind: "run", id: `run:${block.id}`, blocks: [block] });
    }
  }
  // Thinking alone is not a run of work: it keeps its own fold.
  return out.flatMap((item) =>
    item.kind === "run" && !item.blocks.some((b) => b.kind === "tool") ? item.blocks : [item],
  );
}

/** How long one step stays on the folded line before the next may replace it. */
const STEP_MS = 700;

/** What the run is doing right now: the call under way, else the last thing in it. */
function activity(run: Run): string {
  const running = run.blocks.filter((b): b is Tool => b.kind === "tool" && b.status === "running").pop();
  const last = running ?? run.blocks[run.blocks.length - 1];
  return last.kind === "tool" ? describeActive(last) : "Thinking";
}

function ToolRun({
  run,
  live,
  onOpenProcess,
  onOpenAgent,
}: {
  run: Run;
  live: boolean;
  onOpenProcess?: OpenProcess;
  onOpenAgent?: OpenAgent;
}) {
  const tools = run.blocks.filter((b): b is Tool => b.kind === "tool");
  const failed = tools.filter((t) => t.status === "failed").length;
  // Held for a moment each, so quick calls do not flicker past unread.
  const step = useSteadyValue(live ? activity(run) : null, STEP_MS);

  return (
    <details className={`tool-run${live ? " live" : ""}`}>
      <summary>
        {live && <Loader2 className="tool-run-spin" size={12} aria-hidden="true" />}
        {live && step ? (
          <span key={step} className="tool-run-text tool-run-step">
            {step}…
          </span>
        ) : (
          <span className="tool-run-text">{describeRun(tools)}</span>
        )}
        {live && (
          <span className="tool-run-count">
            {tools.length} {tools.length === 1 ? "call" : "calls"}
          </span>
        )}
        {failed > 0 && <span className="tool-run-failed">{failed} failed</span>}
        <ChevronRight className="chev" size={12} />
      </summary>
      <div className="tools">
        {run.blocks.map((block) =>
          block.kind === "tool" ? (
            <ToolRow key={block.id} block={block} onOpenProcess={onOpenProcess} onOpenAgent={onOpenAgent} />
          ) : (
            renderBlock(block, () => {}, false)
          ),
        )}
      </div>
    </details>
  );
}

type Props = {
  /** Whether Changes is showing; the header's button shows and hides it, and only it. */
  asideOpen?: boolean;
  /** Writes the conversation out as a file. One entry of the header's "…" menu. */
  onExport?: () => void;
  onToggleAside?: () => void;
  /** Whether Terminal is showing; its own header button, beside Changes', shows and hides it. */
  terminalOpen?: boolean;
  onToggleTerminal?: () => void;
  /** Shows the side panel on one particular panel — the rest of that menu. */
  onOpenPanel?: (tab: AsideTab) => void;
  /** The open chat's title; `null` for one not saved yet. */
  title?: string | null;
  /** Whether the open chat was branched off another one; its header icon says so. */
  branched?: boolean;
  workspace: string | null;
  turn: TurnState;
  onDecide: (decisions: ToolCallDecision[], always: string[]) => void;
  onOpenRepo: () => void;
  /** Present while the conversation is in Plan mode: hands the plan to Agent mode. */
  onImplement?: () => void;
  /** Opens the Plan tab, where the plan is read and edited before handing it over. */
  onOpenPlan?: () => void;
  /** Opens the Terminal tab on a background process a call started. */
  onOpenProcess?: (id: number) => void;
  /** Opens the Agents tab on the helper run an `explore` call was. */
  onOpenAgent?: OpenAgent;
  /** Background processes still running, newest first — counted beside the turn's clock. */
  runningProcesses?: number[];
  /** Puts a shell block from an answer at a prompt in the Terminal tab, not run. Kept stable: every answer re-renders when it changes. */
  onPasteCommand?: (command: string) => void;
  /** Opens a file an answer links to in the viewer. Kept stable, as `onPasteCommand`. */
  onOpenFile?: (link: string) => void;
  /** Bubbles a branch can start at; `null` while a turn runs. */
  branchable?: ReadonlySet<string> | null;
  onBranch?: (bubbleId: string) => void;
  /** Asks to take this chat, and the files, back to before a bubble — at the same points as a branch. */
  onRewind?: (bubbleId: string) => void;
  /** Puts a request to fix a review's finding into the message box. */
  onFix?: (text: string) => void;
};

type TranscriptProps = Pick<
  Props,
  | "turn"
  | "onDecide"
  | "onOpenProcess"
  | "onOpenAgent"
  | "runningProcesses"
  | "onPasteCommand"
  | "onOpenFile"
  | "branchable"
  | "onBranch"
  | "onRewind"
  | "onFix"
> & {
  /** Who answers, over their turns: the agent, or a chat's role. */
  speaker?: string;
  /** What a card's calls would do; the agent's folder preview when absent. */
  preview?: PreviewCalls;
};

type PreviewCalls = (calls: PendingToolCall[]) => Promise<ToolPreview[]>;

/**
 * The conversation as turns — the user's bubbles, the answers, the calls and
 * their cards. The agent's chat draws it, and Chat mode draws its own with it,
 * so a role's tool calls and approval cards look as the agent's do.
 */
export function Transcript({
  turn,
  onDecide,
  speaker = "Agent",
  preview,
  onOpenProcess,
  onOpenAgent,
  runningProcesses = [],
  onPasteCommand,
  onOpenFile,
  branchable = null,
  onBranch,
  onRewind,
  onFix,
}: TranscriptProps) {
  const groups = group(turn.blocks);
  // The one answer still arriving: the last block of a running turn.
  const streamingId = turn.status === "running" ? turn.blocks[turn.blocks.length - 1]?.id : undefined;
  return (
    <>
      {groups.map((turnGroup, index) => (
        <div className="turn" key={index}>
          {turnGroup.role !== "notice" && (
            <div className={`role${turnGroup.role === "agent" ? " agent" : ""}`}>
              {turnGroup.role === "agent" ? speaker : "You"}
            </div>
          )}
          {fold(turnGroup.blocks).map((block, at, items) =>
            block.kind === "run" ? (
              <ToolRun
                key={block.id}
                run={block}
                // Only the work at the very end is under way; a run the
                // agent has already written past is finished.
                live={turn.status === "running" && index === groups.length - 1 && at === items.length - 1}
                onOpenProcess={onOpenProcess}
                onOpenAgent={onOpenAgent}
              />
            ) : block.kind === "user" ? (
              <UserBubble key={block.id} block={block} branchable={branchable} onBranch={onBranch} onRewind={onRewind} />
            ) : (
              renderBlock(block, onDecide, block.id === streamingId, onOpenProcess, onPasteCommand, onOpenFile, onFix, onOpenAgent, preview)
            ),
          )}
          {workedFooter(
            groups,
            index,
            turn,
            index === groups.length - 1 && <RunningProcesses ids={runningProcesses} onOpen={onOpenProcess} />,
          )}
        </div>
      ))}
      {turn.status === "running" && turn.runningSince !== null && (
        <WorkingClock since={turn.runningSince} before={turnMessage(groups, groups.length - 1)?.workedMs ?? 0}>
          <RunningProcesses ids={runningProcesses} onOpen={onOpenProcess} />
        </WorkingClock>
      )}
    </>
  );
}

export function ChatPanel({
  asideOpen = false,
  onExport,
  onToggleAside,
  terminalOpen = false,
  onToggleTerminal,
  onOpenPanel,
  title = null,
  branched = false,
  workspace,
  turn,
  onDecide,
  onOpenRepo,
  onImplement,
  onOpenPlan,
  onOpenProcess,
  onOpenAgent,
  runningProcesses = [],
  onPasteCommand,
  onOpenFile,
  branchable = null,
  onBranch,
  onRewind,
  onFix,
}: Props) {
  const groups = group(turn.blocks);
  // Under a finished answer only: mid-turn the plan is not written yet, and
  // after a stop or a failure it may be half of one.
  const planReady = onImplement && turn.status === "done" && groups[groups.length - 1]?.role === "agent";
  // The thread follows the answer as it grows, until the user scrolls up to
  // read; scrolling back to the end picks it up again.
  const { scrollRef, contentRef, scrollToBottom } = useFollowBottom();
  // A new message, or another chat, is where the user is looking now —
  // follow it even if they had scrolled away.
  const lastUserId = turn.blocks.filter((block) => block.kind === "user").pop()?.id;
  useEffect(() => {
    scrollToBottom();
  }, [lastUserId, scrollToBottom]);

  // The side panels first — they are what the menu is opened for — then what
  // can be done to the conversation itself.
  const menu: ChatMenuItem[] = [
    ...(onOpenPanel
      ? PANES.filter((p) => !(onToggleTerminal && p.id === "terminal")).map(({ id, label, icon: Icon }) => ({
          id,
          label,
          icon: <Icon size={14} />,
          shortcut: shortcutText(id),
          onSelect: () => onOpenPanel(id),
        }))
      : []),
    ...(onExport
      ? [
          {
            id: "export",
            label: "Export as Markdown…",
            icon: <Download size={14} />,
            divided: Boolean(onOpenPanel),
            onSelect: onExport,
          },
        ]
      : []),
  ];

  return (
    <section className="chat-panel">
      <header className="chat-head">
        <div className="head-left">
          {branched ? (
            <GitBranch size={15} className="head-icon" aria-hidden />
          ) : (
            <MessageSquare size={15} className="head-icon" aria-hidden />
          )}
          <h1 title={title ?? undefined}>{title ?? "New chat"}</h1>
        </div>
        <div className="head-right">
          {turn.retrying && (
            <span className="head-time" title="The provider refused; waiting before trying again">
              retrying in {turn.retrying.delaySeconds}s ({turn.retrying.attempt}/
              {turn.retrying.maxAttempts})
            </span>
          )}
          {menu.length > 0 && <ChatMenu items={menu} />}
          {onToggleTerminal && (
            <button
              type="button"
              className={`iconbtn aside-button${terminalOpen ? " on" : ""}`}
              title={`${terminalOpen ? "Hide terminal" : "Show terminal"} (${shortcutText("terminal")})`}
              aria-pressed={terminalOpen}
              onClick={onToggleTerminal}
            >
              <TerminalSquare size={15} />
            </button>
          )}
          {onToggleAside && (
            <button
              type="button"
              className={`iconbtn aside-button${asideOpen ? " on" : ""}`}
              title={`${asideOpen ? "Hide changes" : "Show changes"} (${shortcutText("changes")})`}
              aria-pressed={asideOpen}
              onClick={onToggleAside}
            >
              <FileDiff size={15} />
            </button>
          )}
        </div>
      </header>

      <div ref={scrollRef} className={`thread chat-text${groups.length === 0 ? " thread-empty" : ""}`}>
        {groups.length === 0 ? (
          <ChatEmptyState workspace={workspace} onOpenRepo={onOpenRepo} />
        ) : (
          <div ref={contentRef}>
            <Transcript
              turn={turn}
              onDecide={onDecide}
              onOpenProcess={onOpenProcess}
              onOpenAgent={onOpenAgent}
              runningProcesses={runningProcesses}
              onPasteCommand={onPasteCommand}
              onOpenFile={onOpenFile}
              branchable={branchable}
              onBranch={onBranch}
              onRewind={onRewind}
              onFix={onFix}
            />
            {planReady && (
              <div className="plan-handoff">
                <button type="button" className="btn btn-primary" onClick={onImplement}>
                  Implement in Agent mode
                </button>
                {onOpenPlan && (
                  <button type="button" className="btn btn-ghost" onClick={onOpenPlan}>
                    Review the plan
                  </button>
                )}
                <span>Switches to Agent with the plan and the checklist.</span>
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

/**
 * What the user said, with a way to try it differently: a branch keeps the
 * conversation up to here in a new chat and gives this text back to edit.
 *
 * Offered at rest only, and not on a message folded into the compaction
 * summary — the model no longer has what came before it. That button stays,
 * disabled, so its absence is not a mystery.
 */
function UserBubble({
  block,
  branchable,
  onBranch,
  onRewind,
}: {
  block: Extract<Block, { kind: "user" }>;
  branchable: ReadonlySet<string> | null;
  onBranch?: (bubbleId: string) => void;
  onRewind?: (bubbleId: string) => void;
}) {
  const offered = branchable !== null && onBranch !== undefined;
  const can = branchable?.has(block.id) ?? false;
  // The action sits under the message, shown on hover in room kept for it,
  // so a long conversation is not a column of buttons and nothing moves.
  return (
    <div className="user-msg">
      <div className="bubble">{block.text}</div>
      <div className="bubble-foot">
        <CopyAction text={block.text} />
        {offered && (
          <button
            type="button"
            className="bubble-action"
            disabled={!can}
            title={
              can
                ? "A new chat with the conversation up to this message, which you can change and send again"
                : "Folded into the summary of earlier conversation — the model no longer sees what came before it, so a branch cannot start here"
            }
            onClick={() => onBranch(block.id)}
          >
            <GitBranch size={12} />
            Branch from here
          </button>
        )}
        {branchable !== null && onRewind && (
          <button
            type="button"
            className="bubble-action"
            disabled={!can}
            title={
              can
                ? "Take this chat back to before this message, and the files the agent changed since, to change it and send again"
                : "Folded into the summary of earlier conversation — the chat cannot be taken back to here"
            }
            onClick={() => onRewind(block.id)}
          >
            <RotateCcw size={12} />
            Rewind
          </button>
        )}
      </div>
    </div>
  );
}

function renderBlock(
  block: Block,
  onDecide: (decisions: ToolCallDecision[], always: string[]) => void,
  streaming: boolean,
  onOpenProcess?: OpenProcess,
  onPasteCommand?: (command: string) => void,
  onOpenFile?: (link: string) => void,
  onFix?: (text: string) => void,
  onOpenAgent?: OpenAgent,
  preview?: PreviewCalls,
) {
  switch (block.kind) {
    case "user":
      return (
        <div className="bubble" key={block.id}>
          {block.text}
        </div>
      );
    case "steer":
      // Looks like any message; what sets it apart is said on hover, in room
      // kept for it so nothing moves.
      return (
        <div className="user-msg" key={block.id}>
          <div className="bubble">{block.text}</div>
          <div className="bubble-foot">
            <CopyAction text={block.text} />
            Sent while the agent was working
          </div>
        </div>
      );
    case "notice":
      return (
        <p className="notice" key={block.id}>
          {block.text}
        </p>
      );
    case "processEnded":
      return <ProcessEndedCard key={block.id} process={block.process} onOpen={onOpenProcess} />;
    case "compaction":
      return <CompactionCard key={block.id} block={block} />;
    case "message":
      return (
        <div className="msg" key={block.id}>
          <Markdown text={block.text} streaming={streaming} onPaste={onPasteCommand} onOpenFile={onOpenFile} />
          {/* Room is kept while it streams, so the button appears without the thread moving. */}
          <div className="bubble-foot">{!streaming && <CopyAction text={block.text} />}</div>
        </div>
      );
    case "reasoning":
      return (
        // Drawn as a call row, so thinking and calls read as one list.
        <details className="tool-item reasoning" key={block.id}>
          <summary className="tool">
            <span className="ico">
              <Brain size={13} />
            </span>
            <span className="name">Thinking</span>
            <span className="arg reasoning-preview">{block.text.split("\n", 1)[0]}</span>
            <ChevronRight className="chev" size={12} />
          </summary>
          <p className="tool-detail reasoning-text">{block.text}</p>
        </details>
      );
    case "tool": {
      // A review's finding is the work's result, not a step of it.
      const finding = findingOf(block);
      if (finding) return <FindingCard key={block.id} finding={finding} onOpenFile={onOpenFile} onFix={onFix} />;
      return (
        <div className="tools" key={block.id}>
          <ToolRow block={block} onOpenProcess={onOpenProcess} onOpenAgent={onOpenAgent} />
        </div>
      );
    }
    case "approval":
      return (
        <div className="tools" key={block.id}>
          <ApprovalCard block={block} onDecide={onDecide} preview={preview} />
        </div>
      );
  }
}
