import { useState } from "react";
import { ChevronRight, Check, Circle, Eye, Loader2, SearchCheck, Wrench, X } from "lucide-react";
import type { Block } from "../lib/chatTurnReducer";
import type { Finding, GroupProgress, GroupSummary, ReviewExclusion } from "../lib/chat";
import { compact } from "./ContextMeter";
import "./ReviewCard.css";

type Props = {
  block: Extract<Block, { kind: "review" }>;
  /** Opens a finding's file in the viewer. */
  onOpenFile?: (link: string) => void;
  /** Puts a request to fix a finding into the message box. */
  onFix?: (text: string) => void;
};

const LEFT_OUT: Record<ReviewExclusion, string> = {
  binary: "binary",
  deleted: "deleted",
  secret: "may hold secrets",
  generated: "generated or a lockfile",
  documentation: "documentation",
  tooLarge: "diff too large",
};

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Model text with `code` in backticks, the code drawn as code. */
export function Prose({ text }: { text: string }) {
  return <>{text.split("`").map((part, i) => (i % 2 === 1 ? <code key={i}>{part}</code> : part))}</>;
}

export const lines = (f: Finding) => (f.startLine === f.endLine ? `${f.startLine}` : `${f.startLine}-${f.endLine}`);

/** A group by the folder its files share: `src/lib · 4 files`, or the one file. */
export function groupName(files: string[]): string {
  if (files.length === 1) return files[0];
  let common = files[0].split("/").slice(0, -1);
  for (const file of files) {
    const parts = file.split("/");
    let same = 0;
    while (same < common.length && common[same] === parts[same]) same++;
    common = common.slice(0, same);
  }
  return common.length ? `${common.join("/")} · ${files.length} files` : `${files.length} files`;
}

/** What a group's worker is doing now, or how it ended. */
function groupStatus(group: GroupProgress): string {
  switch (group.state) {
    case "waiting":
      return "Waiting";
    case "working":
      return group.findings > 0 ? `Reviewing… ${plural(group.findings, "finding")} so far` : "Reviewing…";
    case "done":
      return group.findings === 0 ? "No findings" : plural(group.findings, "finding");
    case "failed":
      return `Not finished — ${group.error ?? "failed"}`;
  }
}

/**
 * Tokens as the provider counted them, or — until it has — the backend's own
 * estimate of the request, marked `~`.
 */
function spent(groups: GroupProgress[]): string | null {
  const counted = groups.reduce((sum, group) => sum + (group.input + group.output || group.estimate), 0);
  if (counted === 0) return null;
  const estimated = groups.some((group) => group.input + group.output === 0 && group.estimate > 0);
  return `${estimated ? "~" : ""}${compact(counted)} tokens`;
}

/**
 * What its one request cost — in and out apart, since the reply's thinking
 * can outweigh the whole diff — and the findings the check sent back.
 */
function groupCost(group: GroupProgress): string {
  const parts = [];
  if (group.input + group.output > 0) parts.push(`${compact(group.input)} in · ${compact(group.output)} out`);
  else if (group.estimate > 0) parts.push(`~${compact(group.estimate)} in`);
  // A finding whose code is not in the change, or names no one place.
  if (group.failedCalls > 0) parts.push(`${group.failedCalls} not placed`);
  return parts.join(" · ");
}

const ICONS = {
  waiting: <Circle size={11} />,
  working: <Loader2 className="review-spin" size={12} />,
  done: <Check size={12} />,
  failed: <X size={12} />,
};

function GroupRow({ group }: { group: GroupProgress }) {
  const cost = groupCost(group);
  return (
    <li className={`review-group ${group.state}`}>
      <span className="review-group-icon">{ICONS[group.state]}</span>
      <div className="review-group-main">
        <div className="review-group-name" title={group.files.join("\n")}>
          {groupName(group.files)}
        </div>
        {group.state !== "waiting" && (
          <div className={`review-group-status${group.state === "working" ? " live" : ""}`}>{groupStatus(group)}</div>
        )}
        {cost && <div className="review-group-cost">{cost}</div>}
        {group.note && <div className="review-group-note">{group.note}</div>}
        {group.truncated && <div className="review-group-note">Cut off at the provider's length limit</div>}
      </div>
    </li>
  );
}

/**
 * One group's closing word, as its own panel: its files and how many
 * findings are in them, what the change does, what was checked (folded — it
 * runs long), and what is worth a look. `summary` is `null` when the worker
 * sent none.
 */
function GroupPanel({
  files,
  summary,
  found,
  truncated,
  onOpenFile,
}: {
  files: string[];
  summary: GroupSummary | null;
  found: number;
  /** Its reply stopped at the length limit — why a summary may be missing. */
  truncated: boolean;
  onOpenFile?: (link: string) => void;
}) {
  return (
    <section className="review-summary">
      <div className="review-summary-head">
        <span className="review-summary-name" title={files.join("\n")}>
          {groupName(files)}
        </span>
        <span className={`review-chip${found > 0 ? " found" : ""}`}>{found === 0 ? "No findings" : plural(found, "finding")}</span>
      </div>
      {summary ? (
        <>
          <p className="review-body">
            <Prose text={summary.summary} />
          </p>
          {summary.worthALook.length > 0 && (
            <div className="review-worth">
              <div className="review-worth-label">
                <Eye size={12} /> Worth a look
              </div>
              <ul>
                {summary.worthALook.map((concern) => (
                  <li key={`${concern.path}:${concern.note}`}>
                    {onOpenFile ? (
                      <button type="button" className="review-link review-path" onClick={() => onOpenFile(concern.path)}>
                        {concern.path}
                      </button>
                    ) : (
                      <span className="review-path">{concern.path}</span>
                    )}
                    <span>
                      <Prose text={concern.note} />
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {summary.checked.length > 0 && (
            <details className="review-checked">
              <summary>
                <ChevronRight className="chev" size={12} />
                Checked {summary.checked.length}
              </summary>
              <ul>
                {summary.checked.map((item) => (
                  <li key={item}>
                    <Prose text={item} />
                  </li>
                ))}
              </ul>
            </details>
          )}
        </>
      ) : (
        <p className="review-muted">
          {truncated
            ? "No summary — the reply was cut off at the provider's length limit before it came."
            : "No summary — the model did not send one for these files."}
        </p>
      )}
    </section>
  );
}

/** `/review`: its groups as their workers go, then what it found, most severe first. */
export function ReviewCard({ block, onOpenFile, onFix }: Props) {
  const [showLeftOut, setShowLeftOut] = useState(false);
  const [showGroups, setShowGroups] = useState(false);
  const { report, groups } = block;
  const tokens = spent(groups);

  if (!report) {
    const total = groups[0]?.total ?? 0;
    const ended = groups.filter((group) => group.state === "done" || group.state === "failed").length;
    return (
      <div className="review-card" role="status">
        <div className="review-head">
          <Loader2 className="review-spin" size={14} />
          <span className="review-title">{total === 0 ? "Reading the changes…" : "Reviewing the working tree…"}</span>
          {total > 0 && (
            <span className="review-meta">
              {ended} of {plural(total, "group")}
              {tokens && ` · ${tokens}`}
            </span>
          )}
        </div>
        <div className="review-bar" aria-hidden="true">
          <div className="review-bar-fill" style={{ width: `${total ? (ended / total) * 100 : 0}%` }} />
        </div>
        {groups.length > 0 && (
          <ul className="review-groups">
            {groups.map((group) => (
              <GroupRow key={group.group} group={group} />
            ))}
          </ul>
        )}
      </div>
    );
  }

  const { findings, reviewed, excluded, failed, summaries } = report;
  // A panel a group: those that sent a summary, and finished ones that did
  // not — named, so a group is never silently missing.
  const panels = [
    ...summaries.map((summary) => ({ files: summary.files, summary })),
    ...groups
      .filter((group) => group.state === "done" && !summaries.some((summary) => summary.files.join() === group.files.join()))
      .map((group) => ({ files: group.files, summary: null })),
  ].sort((a, b) => a.files.join().localeCompare(b.files.join()));
  return (
    <div className="review-card">
      <div className="review-head">
        <SearchCheck size={14} />
        <span className="review-title">
          {reviewed.length === 0 && failed.length === 0
            ? "Nothing to review"
            : findings.length === 0
              ? "No problems found"
              : plural(findings.length, "finding")}
        </span>
        <span className="review-meta">
          {plural(reviewed.length, "file")} reviewed
          {excluded.length > 0 && (
            <>
              {" · "}
              <button type="button" className="review-link" onClick={() => setShowLeftOut((v) => !v)}>
                {excluded.length} left out
              </button>
            </>
          )}
          {groups.length > 0 && (
            <>
              {" · "}
              <button type="button" className="review-link" onClick={() => setShowGroups((v) => !v)}>
                {plural(groups.length, "group")}
              </button>
            </>
          )}
          {tokens && ` · ${tokens}`}
        </span>
      </div>
      {showLeftOut && (
        <ul className="review-left-out">
          {excluded.map((file) => (
            <li key={file.path}>
              <span className="review-path">{file.path}</span> — {LEFT_OUT[file.reason]}
            </li>
          ))}
        </ul>
      )}
      {showGroups && (
        <ul className="review-groups">
          {groups.map((group) => (
            <GroupRow key={group.group} group={group} />
          ))}
        </ul>
      )}
      {findings.length > 0 && (
        <>
          <div className="review-section">Findings</div>
          <ol className="review-findings">
            {findings.map((finding) => (
              <li key={finding.id} className="review-finding">
                <div className="review-finding-head">
                  <span className={`review-severity ${finding.severity}`}>{finding.severity}</span>
                  <span className="review-finding-title">
                    {finding.id}. <Prose text={finding.title} />
                  </span>
                  {onFix && (
                    <button
                      type="button"
                      className="review-fix"
                      title="Ask the agent to fix this"
                      onClick={() => onFix(`Fix finding ${finding.id} from the review: ${finding.title} (${finding.path}:${lines(finding)})`)}
                    >
                      <Wrench size={12} />
                      Fix
                    </button>
                  )}
                </div>
                {onOpenFile ? (
                  <button type="button" className="review-link review-path" onClick={() => onOpenFile(`${finding.path}:${finding.startLine}`)}>
                    {finding.path}:{lines(finding)}
                  </button>
                ) : (
                  <span className="review-path">
                    {finding.path}:{lines(finding)}
                  </span>
                )}
                <p className="review-body">
                  <Prose text={finding.body} />
                </p>
                {finding.suggestion && <pre className="review-suggestion">{finding.suggestion}</pre>}
              </li>
            ))}
          </ol>
        </>
      )}
      {panels.length > 0 && (
        <>
          <div className="review-section">By group</div>
          <div className="review-summaries">
            {panels.map(({ files, summary }) => (
              <GroupPanel
                key={files.join()}
                files={files}
                summary={summary}
                found={findings.filter((finding) => files.includes(finding.path)).length}
                truncated={groups.some((group) => group.truncated && group.files.join() === files.join())}
                onOpenFile={onOpenFile}
              />
            ))}
          </div>
        </>
      )}
      {failed.map((group) => (
        <p key={group.files.join()} className="review-failed">
          Not finished — {group.error}: {group.files.join(", ")}
        </p>
      ))}
    </div>
  );
}
