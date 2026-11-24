import { GitCompare, MessagesSquare } from "lucide-react";
import { addressCommentsRequest, resolveConflictsRequest, type PrTasks } from "../lib/pullRequestView";
import "./PrTaskCards.css";

/**
 * The pull request's tasks under the agent's answer (docs/62-bitbucket-prs.md,
 * BB-8): Resolve conflicts while the server says conflicted, Address comments
 * while remarks are open. Each puts its request into the message box — the
 * user reads it and sends, as a review's Fix does; nothing leaves on the press.
 */
export function PrTaskCards({ tasks, onTask }: { tasks: PrTasks; onTask: (text: string) => void }) {
  return (
    <section className="pr-task-cards" aria-label="Pull request next steps">
      <span className="pr-task-cards-caption" aria-hidden="true">
        Pull request next steps
      </span>
      {tasks.conflicted && (
        <button
          type="button"
          className="pr-task-card conflicts"
          title={`Merge conflicts with ${tasks.pr.toBranch}`}
          onClick={() => onTask(resolveConflictsRequest(tasks.pr))}
        >
          <span className="pr-task-card-icon">
            <GitCompare size={14} />
          </span>
          <span className="pr-task-card-label">Resolve conflicts</span>
        </button>
      )}
      {tasks.comments.length > 0 && (
        <button
          type="button"
          className="pr-task-card"
          title={`${tasks.comments.length} open review ${tasks.comments.length === 1 ? "remark" : "remarks"}`}
          onClick={() => onTask(addressCommentsRequest(tasks.pr, tasks.comments))}
        >
          <span className="pr-task-card-icon">
            <MessagesSquare size={14} />
          </span>
          <span className="pr-task-card-label">Address comments</span>
        </button>
      )}
    </section>
  );
}