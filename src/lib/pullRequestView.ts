import type { BuildState, Reviewer } from "./pullRequests";

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
