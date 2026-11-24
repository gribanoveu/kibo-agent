import type { Block } from "./chatTurnReducer";

// An offer is not state of its own either: it is an `offer` call the backend
// accepted — its arguments say what and why. It is drawn after the answer,
// not where the call was made.

export type OfferKind = "review" | "commit" | "worktree" | "pullRequest" | "syncBase" | "cleanup" | "plan" | "message";

export type Offer = { kind: OfferKind; reason: string; text?: string };

/** What pressing a commit offer sends. */
export const COMMIT_REQUEST = "Commit the changes.";
/** What pressing a review offer sends: the review, then the agent goes through the findings. */
export const REVIEW_REQUEST =
  "Review the change with the `review` tool and show the findings, then fix what you recognize as real and say what you dismissed and why. Do not change files beyond those fixes.";
/** A PR is requested, never pushed by the offer button itself. */
export const PULL_REQUEST = "Create a pull request from this worktree branch into its recorded base. Push and create it with confirmed commands.";
/** The same with Pull requests on: the app's own tool opens it on the user's server, through its card. */
export const PULL_REQUEST_ON_SERVER =
  "Create a pull request from this worktree branch into its recorded base. Push the branch with a confirmed command, then open the pull request with `pullRequestCreate` (find it with `toolSearch`) — not `gh` or `curl`. If it says no server is configured for this remote, create it with confirmed commands instead.";
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
      kind !== "plan" &&
      kind !== "message")
  ) {
    return null;
  }
  let args: Partial<Record<string, unknown>>;
  try {
    args = JSON.parse(block.arguments);
  } catch {
    return null;
  }
  const reason = typeof args.reason === "string" ? args.reason.trim() : "";
  if (kind === "message") {
    // The suggestion is its text; without it there is nothing to send.
    const text = typeof args.text === "string" ? args.text.trim() : "";
    return text ? { kind, reason, text } : null;
  }
  return { kind, reason };
}

/** A turn's offers: one per fixed kind (a kind offered again keeps its place
    and takes the later reason), and the last two message suggestions — keyed
    by text, so the same suggestion twice is one card. */
export function offersOf(blocks: Block[]): Offer[] {
  const byKind = new Map<OfferKind, Offer>();
  const messages = new Map<string, Offer>();
  for (const block of blocks) {
    const offer = offerOf(block);
    if (!offer) continue;
    if (offer.kind === "message") {
      // Delete first: a suggestion offered again is the latest one, even if
      // its text was already pushed out of the two — Map.set alone would
      // keep its old place and the press would never show it.
      messages.delete(offer.text ?? "");
      messages.set(offer.text ?? "", offer);
    } else byKind.set(offer.kind, offer);
  }
  return [...byKind.values(), ...[...messages.values()].slice(-2)];
}
