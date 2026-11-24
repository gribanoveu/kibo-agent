import type { BuildState, PrComment, PrStrip, PullRequest, Reviewer } from "./pullRequests";

// What the Overview rows say in a word.

const BUILD_TEXT: Record<BuildState, string> = {
  successful: "Build passed",
  failed: "Build failed",
  inProgress: "Build in progress",
  unknown: "Build status unknown",
};

/** The build row's text: what the latest build is doing. */
export const buildText = (state: BuildState): string => BUILD_TEXT[state];

/** "1/2 approved", "needs work" when anyone said so, `null` with no reviewers. */
export function reviewsAtAGlance(reviewers: Reviewer[]): string | null {
  if (reviewers.length === 0) return null;
  if (reviewers.some((r) => r.status === "needsWork")) return "needs work";
  const approved = reviewers.filter((r) => r.status === "approved").length;
  return `${approved}/${reviewers.length} approved`;
}

/** The line under the pull request: whether it can go in — `label` on the line, `text` under the pointer. */
export function readiness(draft: boolean, mergeBlocked: string[] | null): { kind: "draft" | "ready" | "blocked"; label: string; text: string } {
  if (draft) return { kind: "draft", label: "Not ready for review", text: "Draft — mark it ready to merge" };
  if (mergeBlocked === null) return { kind: "ready", label: "Ready to merge", text: "Ready to merge" };
  return {
    kind: "blocked",
    label: "Cannot merge yet",
    text: mergeBlocked.length > 0 ? `Cannot merge yet: ${mergeBlocked.join("; ")}` : "Cannot merge yet",
  };
}

const REVIEW_ORDER: Record<Reviewer["status"], number> = { needsWork: 0, unapproved: 1, approved: 2 };

/** Reviewers as the opened pull request lists them: who holds it up first — needs work, then not yet — and those done last. */
export function reviewersInOrder(reviewers: Reviewer[]): Reviewer[] {
  return [...reviewers].sort((a, b) => REVIEW_ORDER[a.status] - REVIEW_ORDER[b.status]);
}

/** A reviewer's status in words, at the end of their line. */
export const REVIEW_WORD: Record<Reviewer["status"], string> = { approved: "approved", needsWork: "needs work", unapproved: "not yet" };

/** Hosts that are never a Bitbucket Data Center: a folder cloned from one is not offered Connect. */
const NOT_BITBUCKET = ["github.com", "gitlab.com"];

/** Whether a remote on `host` is offered to connect a Bitbucket server — any host but GitHub's and GitLab's own. */
export function offersConnect(host: string): boolean {
  const name = host.toLowerCase();
  return !NOT_BITBUCKET.some((public_) => name === public_ || name.endsWith(`.${public_}`));
}

// ─── The pull request's tasks for the agent (BB-8) ─────────────────────────

/** What the pull request asks of the agent: conflicts to resolve, open remarks to address. */
export type PrTasks = {
  pr: PullRequest;
  conflicted: boolean;
  /** The open remarks only: a resolved thread or task is not work. */
  comments: PrComment[];
};

/** The tasks of a strip's state — `null` when there is nothing to do: no pull
    request, no conflict and no open remark. A failed build is not a task here:
    its logs are in CI, and Fix CI is not offered (decision 10). */
export function prTasksOf(strip: PrStrip | null): PrTasks | null {
  if (!strip || strip.state !== "ready" || !strip.snapshot.pr) return null;
  const conflicted = strip.snapshot.merge?.conflicted === true;
  const comments = strip.snapshot.comments.filter((comment) => !comment.resolved);
  if (!conflicted && comments.length === 0) return null;
  return { pr: strip.snapshot.pr, conflicted, comments };
}

/** What pressing Resolve conflicts puts in the message box: the target named. */
export function resolveConflictsRequest(pr: PullRequest): string {
  return `Pull request #${pr.id} has merge conflicts with \`${pr.toBranch}\`. Bring \`${pr.fromBranch}\` up to date with \`${pr.toBranch}\`, resolve the conflicts, run the checks and push the result.`;
}

/** Most remarks carried into the message; the rest the model reads with the tool. */
const ADDRESSED = 20;

/** What pressing Address comments puts in the box: each open remark with its
    place and author, and the review's own rule — fix the real, dismiss the rest. */
export function addressCommentsRequest(pr: PullRequest, comments: PrComment[]): string {
  const lines = comments.slice(0, ADDRESSED).map((comment) => {
    const place = comment.path ? (comment.line != null ? `${comment.path}:${comment.line}` : comment.path) : "general";
    return `- ${comment.task ? "task" : "comment"} — ${place} — ${comment.authorName}: ${comment.text}`;
  });
  const rest = comments.length - lines.length;
  const more = rest > 0 ? `\n… and ${rest} more — read them with pullRequestStatus.` : "";
  return `Address the open review comments on pull request #${pr.id}:\n${lines.join("\n")}${more}\nFor each, fix what you recognize as real and say what you dismissed and why.`;
}
