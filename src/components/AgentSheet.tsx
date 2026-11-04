import { useEffect, useRef } from "react";
import { Bot, ChevronRight, Square } from "lucide-react";
import { Markdown } from "./Markdown";
import { PageSheet } from "./Page";
import { useAgents } from "../hooks/useAgents";
import { type AgentInfo, agentStatus, briefTitle, splitStep, tokensSpent } from "../lib/agents";
import type { AgentFocus } from "../lib/describeTool";
import "./AgentSheet.css";

/** The tools a run called, oldest first, the one under way marked while it runs; kept scrolled to the newest. */
function Steps({ steps, running }: { steps: string[]; running: boolean }) {
  const list = useRef<HTMLOListElement>(null);
  useEffect(() => {
    const node = list.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [steps.length]);
  return (
    <ol className="agent-steps" ref={list}>
      {steps.map((step, i) => {
        const { tool, on } = splitStep(step);
        const now = running && i === steps.length - 1;
        return (
          <li key={i} className={now ? "now" : undefined} title={step}>
            <b>{tool}</b>
            {on && <span>{on}</span>}
          </li>
        );
      })}
    </ol>
  );
}

/** One helper run in full: the brief it was given, folded, how it stands and
    what it cost, the tools it called — the last one what it is doing now — and
    what it answered, with Stop while it runs. */
export function AgentRun({
  agent: a,
  onStop,
  onOpenFile,
}: {
  agent: AgentInfo;
  onStop: (id: number) => void;
  /** A file the answer names, as the chat opens one; without it the link is plain text. */
  onOpenFile?: (link: string) => void;
}) {
  const running = a.state.state === "running";
  const spent = tokensSpent(a.tokens);
  return (
    <div className={`agent-detail ${a.state.state}`}>
      {/* The brief is a prompt, not an answer — a review's wraps a whole diff in
          `<changes>`, which Markdown would take for HTML and lose — so it is
          shown as written. Folded, so the panel opens on the run itself. */}
      <details className="agent-brief">
        <summary title="What the agent asked it, as written">
          <ChevronRight className="chev" size={12} aria-hidden />
          Brief
        </summary>
        <div className="agent-task">{a.task}</div>
      </details>
      <div className="agent-meta">
        <span className="agent-id">#{a.id}</span>
        <span className="agent-state">{agentStatus(a.state)}</span>
        <span>
          {a.steps.length} {a.steps.length === 1 ? "step" : "steps"}
        </span>
        {spent && <span>{spent}</span>}
        {running && (
          <button type="button" className="agent-detail-stop" title="Stop this helper — the agent carries on without its answer" onClick={() => onStop(a.id)}>
            <Square size={9} fill="currentColor" aria-hidden />
            Stop
          </button>
        )}
      </div>
      {a.steps.length > 0 && <Steps steps={a.steps} running={running} />}
      {running && !a.steps.length && <div className="agent-waiting">Thinking…</div>}
      {a.state.state === "failed" && <div className="agent-reason">{a.state.reason}</div>}
      {a.answer && (
        <div className="agent-answer">
          <Markdown text={a.answer} streaming={false} onOpenFile={onOpenFile} />
        </div>
      )}
    </div>
  );
}

type Props = {
  /** The chat the run belongs to: only its own are listed. */
  chatId: string | null;
  /** Which run: the task as well as the number, which starts again with the app.
      The task may be only the start of the brief — the review card knows no more. */
  run: AgentFocus;
  onClose: () => void;
  /** A link to a file in the answer opens it, as one in the chat does. */
  onOpenFile?: (link: string) => void;
};

/** A helper run opened from the chat, Overview or the Agents tab, in a sheet
    over the chat's right side. Read while it is open, and again each time the
    backend says a run changed, so a running one moves on under the user's eyes. */
export function AgentSheet({ chatId, run, onClose, onOpenFile }: Props) {
  const { agents, loaded, error, stop } = useAgents(true, chatId);
  const agent = agents.find((a) => a.id === run.id && a.task.startsWith(run.task));
  return (
    <PageSheet icon={Bot} title="Agent" label={briefTitle(run.task)} onClose={onClose}>
      {agent ? (
        <AgentRun agent={agent} onStop={stop} onOpenFile={onOpenFile} />
      ) : (
        <div className="agent-detail">
          <div className="agent-task">{run.task}</div>
          {(error || loaded) && <div className="agent-waiting">{error ?? "This run is no longer listed."}</div>}
        </div>
      )}
    </PageSheet>
  );
}
