import { Wrench } from "lucide-react";
import type { Finding } from "../lib/finding";
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
