import type { Block } from "./chatTurnReducer";

// An offer is not state of its own either: it is an `offer` call the backend
// accepted — its arguments say what and why. It is drawn after the answer,
// not where the call was made.

export type OfferKind = "review" | "commit" | "worktree" | "pullRequest" | "implement";

export type Offer = { kind: OfferKind; reason: string };

/** What pressing a commit offer sends. */
export const COMMIT_REQUEST = "Commit the changes.";
/** What pressing a review offer sends: the review, then the agent goes through the findings. */
export const REVIEW_REQUEST =
  "Review the change with the `review` tool and show the findings, then fix what you recognize as real and say what you dismissed and why. Do not change files beyond those fixes.";
/** A PR is requested, never pushed by the offer button itself. */
export const PULL_REQUEST = "Create a pull request from this worktree branch into its recorded base. Push and create it with confirmed commands.";

/** The offer a settled `offer` call made; `null` for any other block, or one refused. */
export function offerOf(block: Block): Offer | null {
  if (block.kind !== "tool" || block.name !== "offer") return null;
  // A refused or unfinished call has no result to say it was offered.
  const result = block.result as { result?: string; kind?: string } | undefined;
  if (
    result?.result !== "offered" ||
    (result.kind !== "review" &&
      result.kind !== "commit" &&
      result.kind !== "worktree" &&
      result.kind !== "pullRequest" &&
      result.kind !== "implement")
  ) {
    return null;
  }
  let args: Partial<Record<string, unknown>>;
  try {
    args = JSON.parse(block.arguments);
  } catch {
    return null;
  }
  return { kind: result.kind, reason: typeof args.reason === "string" ? args.reason.trim() : "" };
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
