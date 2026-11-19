import type { Build, BuildState, Reviewer } from "./pullRequests";

// What the collapsed strip says in a word: the builds of the last commit as
// one state, the reviewers as a count.

/** The state that matters most: a failure over a running build over an unknown over a success. `null` with no builds. */
export function buildsAtAGlance(builds: Build[]): BuildState | null {
  for (const state of ["failed", "inProgress", "unknown", "successful"] as const) {
    if (builds.some((build) => build.state === state)) return state;
  }
  return null;
}

/** "1/2" approved, "needs work" when anyone said so, `null` with no reviewers. */
export function reviewsAtAGlance(reviewers: Reviewer[]): string | null {
  if (reviewers.length === 0) return null;
  if (reviewers.some((r) => r.status === "needsWork")) return "needs work";
  const approved = reviewers.filter((r) => r.status === "approved").length;
  return `${approved}/${reviewers.length}`;
}
