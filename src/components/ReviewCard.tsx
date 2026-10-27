import { Bot, SearchCheck, Wrench } from "lucide-react";
import type { Finding, Review } from "../lib/finding";
import "./ReviewCard.css";

type Props = {
  finding: Finding;
  /** Opens the finding's file in the viewer, at its line. */
  onOpenFile?: (link: string) => void;
  /** Puts a request to fix the finding into the message box. */
  onFix?: (text: string) => void;
};

/** Model text with `code` in backticks, the code drawn as code. */
export function Prose({ text }: { text: string }) {
  return <>{text.split("`").map((part, i) => (i % 2 === 1 ? <code key={i}>{part}</code> : part))}</>;
}

export const lines = (f: Finding) => (f.startLine === f.endLine ? `${f.startLine}` : `${f.startLine}-${f.endLine}`);

/** One problem a review reported, where it stands in the turn: what, where, why, and Fix. */
export function FindingCard({ finding, onOpenFile, onFix }: Props) {
  const place = `${finding.path}:${lines(finding)}`;
  return (
    <div className="review-finding">
      <div className="review-finding-head">
        <span className={`review-severity ${finding.severity}`}>{finding.severity}</span>
        <span className="review-finding-title">
          <Prose text={finding.title} />
        </span>
        {onFix && (
          <button
            type="button"
            className="review-fix"
            title="Ask the agent to fix this"
            onClick={() => onFix(`Fix the finding from the review: ${finding.title} (${place})`)}
          >
            <Wrench size={12} />
            Fix
          </button>
        )}
      </div>
      {onOpenFile ? (
        <button type="button" className="review-link review-path" onClick={() => onOpenFile(`${finding.path}:${finding.startLine}`)}>
          {place}
        </button>
      ) : (
        <span className="review-path">{place}</span>
      )}
      <p className="review-body">
        <Prose text={finding.body} />
      </p>
      {finding.suggestion && <pre className="review-suggestion">{finding.suggestion}</pre>}
    </div>
  );
}

/**
 * A review a subagent ran, as one card: what it found, each finding with
 * Fix when they are the user's — not when they went on to the agent, which
 * answers for them itself — and what the reviewer said.
 */
export function ReviewReport({
  review,
  onOpenFile,
  onFix,
  onOpenAgent,
}: {
  review: Review;
  onOpenFile?: (link: string) => void;
  onFix?: (text: string) => void;
  onOpenAgent?: (agent: { id: number; task: string }) => void;
}) {
  const count = review.findings.length;
  return (
    <section className="review-report" aria-label="Review">
      <div className="review-report-head">
        <span className="review-report-icon">
          <SearchCheck size={14} />
        </span>
        <span className="review-report-title">Review</span>
        <span className="review-report-meta">
          {count === 0 ? "no findings" : count === 1 ? "1 finding" : `${count} findings`}
          {review.handed && " · handed to the agent"}
        </span>
        {onOpenAgent && (
          <button
            type="button"
            className="review-fix"
            title="Open the subagent in the Agents tab"
            onClick={() => onOpenAgent({ id: review.agent, task: "Review the uncommitted changes" })}
          >
            <Bot size={12} />
            Subagent
          </button>
        )}
      </div>
      {review.summary && (
        <p className="review-report-summary">
          <Prose text={review.summary} />
        </p>
      )}
      {review.findings.map((finding) => (
        <FindingCard
          key={`${finding.path}:${finding.startLine}:${finding.title}`}
          finding={finding}
          onOpenFile={onOpenFile}
          onFix={review.handed ? undefined : onFix}
        />
      ))}
    </section>
  );
}
