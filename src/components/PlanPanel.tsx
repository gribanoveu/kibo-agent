import { useEffect, useState } from "react";
import { ChevronRight, Circle, CircleCheck, CircleDot, CircleX, Pencil } from "lucide-react";
import type { Task } from "../lib/chat";
import { Markdown } from "./Markdown";
import "./PlanPanel.css";
import { matches } from "../lib/shortcuts";

type Props = {
  plan: string | null;
  checklist: Task[];
  /** Called when an edit is finished — on leaving the text, not per keystroke. */
  onEdit: (plan: string) => void;
  /** Present in Plan mode once there is something to hand over. */
  onImplement?: () => void;
  /** A running turn may be rewriting the plan; an edit then would be lost. */
  locked: boolean;
};

const MARK: Record<Task["status"], { icon: typeof Circle; label: string }> = {
  completed: { icon: CircleCheck, label: "Done" },
  inProgress: { icon: CircleDot, label: "In progress" },
  pending: { icon: Circle, label: "To do" },
  cancelled: { icon: CircleX, label: "Dropped" },
};

/** The task the work is on: the one in progress, else the next to do; `null`
    once nothing is left. */
export function currentTask(tasks: Task[]): Task | null {
  return tasks.find((t) => t.status === "inProgress") ?? tasks.find((t) => t.status === "pending") ?? null;
}

function TaskRow({ task, current = false }: { task: Task; current?: boolean }) {
  const { icon: Icon, label } = MARK[task.status];
  return (
    <li className={`plan-task ${task.status}${current ? " plan-current" : ""}`}>
      <Icon size={14} className="plan-mark" aria-label={label} />
      <div className="plan-task-body">
        <div className="plan-task-title">{task.title}</div>
        {task.note && <div className="plan-task-note">{task.note}</div>}
      </div>
    </li>
  );
}

/** Folded by default to the one task the work is on — what is worth a glance
    while the agent works; the whole list is a click away. */
function Checklist({ tasks }: { tasks: Task[] }) {
  const [open, setOpen] = useState(false);
  const done = tasks.filter((t) => t.status === "completed").length;
  const current = currentTask(tasks);
  return (
    <div className="panel-section plan-section">
      <div className="section-label">
        <button type="button" className="plan-fold" aria-expanded={open} onClick={() => setOpen(!open)}>
          <ChevronRight size={12} className="plan-fold-chev" />
          Checklist
        </button>
        <span className="count">
          {done}/{tasks.length}
        </span>
      </div>
      <div className="plan-progress" aria-hidden>
        <div style={{ width: `${(done / tasks.length) * 100}%` }} />
      </div>
      <ul className="plan-checklist">
        {open ? (
          tasks.map((task) => <TaskRow key={task.id} task={task} />)
        ) : current ? (
          // Keyed by the task, so a new one is a new row and plays its entrance.
          <TaskRow key={current.id} task={current} current />
        ) : (
          <li key="done" className="plan-task plan-current completed">
            <CircleCheck size={14} className="plan-mark" aria-label="Done" />
            <div className="plan-task-title">All done</div>
          </li>
        )}
      </ul>
    </div>
  );
}

/**
 * The conversation's plan: a document the user reads — drawn as Markdown over
 * the whole panel — and corrects on demand, and the checklist the agent works
 * through. The checklist comes first: it is short, and it is what changes
 * while the agent works. Until the agent makes one it is not shown at all,
 * and the plan has the whole panel.
 */
export function PlanPanel({ plan, checklist, onEdit, onImplement, locked }: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(plan ?? "");
  // A new plan from the agent, or another chat opened, replaces the draft.
  useEffect(() => setDraft(plan ?? ""), [plan]);

  const save = () => draft !== (plan ?? "") && onEdit(draft);
  const finish = () => {
    save();
    setEditing(false);
  };

  return (
    <div className="plan-panel">
      {checklist.length > 0 && <Checklist tasks={checklist} />}

      <div className="panel-section plan-section plan-doc-section">
        <div className="section-label">
          <span>Plan</span>
          {editing ? (
            <button type="button" className="link-btn" onClick={finish}>
              Done
            </button>
          ) : (
            <button
              type="button"
              className="iconbtn plan-edit"
              title={locked ? "The agent is working on the plan" : "Edit plan"}
              aria-label="Edit plan"
              disabled={locked}
              onClick={() => setEditing(true)}
            >
              <Pencil size={12} />
            </button>
          )}
        </div>
        {editing ? (
          <textarea
            className="plan-text"
            aria-label="Plan"
            autoFocus
            placeholder="Write the plan in Markdown."
            value={draft}
            readOnly={locked}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={save}
            onKeyDown={(e) => matches(e, "close") && finish()}
          />
        ) : plan ? (
          <div className="plan-doc">
            <Markdown text={plan} streaming={false} />
          </div>
        ) : (
          <div className="empty">
            No plan yet. In Plan mode the agent writes one here — or{" "}
            <button type="button" className="link-btn" disabled={locked} onClick={() => setEditing(true)}>
              write your own
            </button>
            .
          </div>
        )}
      </div>

      {onImplement && plan && (
        <button type="button" className="btn btn-primary plan-implement" disabled={locked} onClick={onImplement}>
          Implement in Agent mode
        </button>
      )}
    </div>
  );
}
