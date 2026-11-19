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

/** "1/2" approved, "needs work" when anyone said so, `null` with no reviewers. */
export function reviewsAtAGlance(reviewers: Reviewer[]): string | null {
  if (reviewers.length === 0) return null;
  if (reviewers.some((r) => r.status === "needsWork")) return "needs work";
  const approved = reviewers.filter((r) => r.status === "approved").length;
  return `${approved}/${reviewers.length}`;
}

/** Hosts that are never a Bitbucket Data Center: a folder cloned from one is not offered Connect. */
const NOT_BITBUCKET = ["github.com", "gitlab.com"];

/** Whether a remote on `host` is offered to connect a Bitbucket server — any host but GitHub's and GitLab's own. */
export function offersConnect(host: string): boolean {
  const name = host.toLowerCase();
  return !NOT_BITBUCKET.some((public_) => name === public_ || name.endsWith(`.${public_}`));
}
