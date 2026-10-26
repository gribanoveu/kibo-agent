import { useEffect, useRef, useState } from "react";
import { ChevronRight, Square } from "lucide-react";
import { Markdown } from "./Markdown";
import { type AgentInfo, agentStatus, briefTitle, tokensSpent } from "../lib/agents";
import type { AgentFocus } from "../lib/describeTool";
import "./AgentList.css";

type Props = {
  agents: AgentInfo[];
  error: string | null;
  onStop: (id: number) => void;
  /** A run asked for from the chat: it opens and scrolls into view — if it is
   * still this run, the same number and task. A new object each ask. */
  focus?: AgentFocus | null;
};

/** A step as the helper logged it, `readFile src/auth.rs`: the tool, then what it was called on. */
const splitStep = (step: string) => {
  const space = step.indexOf(" ");
  return space < 0 ? { tool: step, on: "" } : { tool: step.slice(0, space), on: step.slice(space + 1) };
};

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

/** The helper agents, a line each: what it was asked, how far it got. Open, the
    whole brief, the tools it called — the last one what it is doing now — what
    it cost, and what it answered. */
export function AgentList({ agents, error, onStop, focus = null }: Props) {
  // Closed until asked: a click, or a run asked for from the chat.
  const [open, setOpen] = useState<number | null>(null);
  const cards = useRef(new Map<number, HTMLDivElement>());

  // Taken once per ask, when the run is in the list — which may be read only
  // after the tab opened for it. A list that changes later does not pull it
  // open again.
  const taken = useRef<AgentFocus | null>(null);
  useEffect(() => {
    if (!focus || taken.current === focus) return;
    if (!agents.some((a) => a.id === focus.id && a.task === focus.task)) return;
    taken.current = focus;
    setOpen(focus.id);
    cards.current.get(focus.id)?.scrollIntoView?.({ block: "nearest" });
  }, [focus, agents]);

  if (!agents.length) {
    return (
      <div className="agent-list-empty">
        {error ??
          "No helper agents yet. The agent hands a research question to one with explore — it reads in a context of its own and answers back."}
      </div>
    );
  }
  return (
    <div className="agent-list">
      {error && <div className="agent-list-error">{error}</div>}
      {agents.map((a) => {
        const running = a.state.state === "running";
        const isOpen = open === a.id;
        const spent = tokensSpent(a.tokens);
        // What it is doing now: the last tool it called.
        const now = a.steps[a.steps.length - 1];
        return (
          <div
            key={a.id}
            ref={(node) => {
              if (node) cards.current.set(a.id, node);
              else cards.current.delete(a.id);
            }}
            className={`agent-run ${a.state.state}${isOpen ? " open" : ""}`}
          >
            <div className="agent-head">
              <button
                type="button"
                className="agent-open"
                aria-expanded={isOpen}
                title={isOpen ? undefined : a.task}
                onClick={() => setOpen(isOpen ? null : a.id)}
              >
                <span className="agent-dot" aria-hidden />
                <span className="agent-title">{briefTitle(a.task)}</span>
                <span className="agent-now">{running ? (now ? splitStep(now).tool : "thinking") : agentStatus(a.state)}</span>
                <ChevronRight className="agent-chev" size={13} aria-hidden />
              </button>
              {running && (
                <button
                  type="button"
                  className="agent-stop"
                  title="Stop this helper — the agent carries on without its answer"
                  aria-label="Stop"
                  onClick={() => onStop(a.id)}
                >
                  <Square size={9} fill="currentColor" aria-hidden />
                </button>
              )}
            </div>
            {isOpen && (
              <div className="agent-detail">
                <div className="agent-task">{a.task}</div>
                <div className="agent-meta">
                  <span className="agent-id">#{a.id}</span>
                  <span className="agent-state">{agentStatus(a.state)}</span>
                  <span>
                    {a.steps.length} {a.steps.length === 1 ? "step" : "steps"}
                  </span>
                  {spent && <span>{spent}</span>}
                </div>
                {a.steps.length > 0 && <Steps steps={a.steps} running={running} />}
                {running && !a.steps.length && <div className="agent-waiting">Thinking…</div>}
                {a.state.state === "failed" && <div className="agent-reason">{a.state.reason}</div>}
                {a.answer && (
                  <div className="agent-answer">
                    <Markdown text={a.answer} streaming={false} />
                  </div>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
