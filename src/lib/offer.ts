import type { Block } from "./chatTurnReducer";

// An offer is not state of its own either: it is an `offer` call the backend
// accepted — its arguments say what and why. It is drawn after the answer,
// not where the call was made.

export type OfferKind = "review" | "commit" | "worktree" | "pullRequest" | "syncBase" | "cleanup" | "plan";

export type Offer = { kind: OfferKind; reason: string };

/** What pressing a commit offer sends. */
export const COMMIT_REQUEST = "Commit the changes.";
/** What pressing a review offer sends: the review, then the agent goes through the findings. */
export const REVIEW_REQUEST =
  "Review the change with the `review` tool and show the findings, then fix what you recognize as real and say what you dismissed and why. Do not change files beyond those fixes.";
/** A PR is requested, never pushed by the offer button itself. */
export const PULL_REQUEST = "Create a pull request from this worktree branch into its recorded base. Push and create it with confirmed commands.";
/** What pressing a syncBase offer sends: the worktree branch is behind its recorded base. */
export const SYNC_BASE =
  "This worktree branch is behind its recorded base. Bring it up to date from that base with confirmed commands, resolve any conflicts, and re-run the checks.";

/** The offer a settled `offer` call made; `null` for any other block, or one refused. */
export function offerOf(block: Block): Offer | null {
  if (block.kind !== "tool" || block.name !== "offer") return null;
  // A refused or unfinished call has no result to say it was offered.
  const result = block.result as { result?: string; kind?: string } | undefined;
  // Chats saved before the rename carry `implement`; it is the plan card now.
  const kind = result?.kind === "implement" ? "plan" : result?.kind;
  if (
    result?.result !== "offered" ||
    (kind !== "review" &&
      kind !== "commit" &&
      kind !== "worktree" &&
      kind !== "pullRequest" &&
      kind !== "syncBase" &&
      kind !== "cleanup" &&
      kind !== "plan")
  ) {
    return null;
  }
  let args: Partial<Record<string, unknown>>;
  try {
    args = JSON.parse(block.arguments);
  } catch {
    return null;
  }
  return { kind, reason: typeof args.reason === "string" ? args.reason.trim() : "" };
}

/** A turn's offers, one per kind: a kind offered again keeps its place and takes the later reason. */
export function offersOf(blocks: Block[]): Offer[] {
  const byKind = new Map<OfferKind, Offer>();
  for (const block of blocks) {
    const offer = offerOf(block);
    if (offer) byKind.set(offer.kind, offer);
  }
  return [...byKind.values()];
}
