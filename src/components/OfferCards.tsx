import { CornerDownLeft, GitCommitHorizontal, GitFork, GitPullRequest, ListChecks, MessageSquarePlus, RefreshCw, SearchCheck, Trash2 } from "lucide-react";
import type { Offer, OfferKind } from "../lib/offer";
import "./OfferCards.css";

const LOOK: Record<OfferKind, { label: string; icon: typeof SearchCheck }> = {
  review: { label: "Run a subagent for review", icon: SearchCheck },
  commit: { label: "Commit", icon: GitCommitHorizontal },
  worktree: { label: "Move this chat to a worktree", icon: GitFork },
  pullRequest: { label: "Create a pull request", icon: GitPullRequest },
  syncBase: { label: "Update from the base branch", icon: RefreshCw },
  cleanup: { label: "Remove the worktree", icon: Trash2 },
  plan: { label: "Open the plan", icon: ListChecks },
  // A message offer's label is its own text; this one is never shown.
  message: { label: "", icon: MessageSquarePlus },
};

/** The next steps the agent offered, after its answer: each a button that takes it — a message sent, or the plan opened. */
export function OfferCards({
  offers,
  onPick,
}: {
  offers: Offer[];
  onPick: (offer: Offer) => void;
}) {
  if (offers.length === 0) return null;
  return (
    <section className="offer-cards" aria-label="Suggested next steps">
      <span className="offer-cards-caption" aria-hidden="true">
        {offers.length === 1 ? "Suggested next step" : "Suggested next steps"}
      </span>
      {offers.map((offer) => {
        const { label, icon: Icon } = LOOK[offer.kind];
        const title = offer.kind === "message" ? (offer.text ?? label) : label;
        return (
          <button
            key={offer.kind === "message" ? `message:${offer.text}` : offer.kind}
            type="button"
            className={`offer-card ${offer.kind}`}
            onClick={() => onPick(offer)}
          >
            <span className="offer-card-icon">
              <Icon size={14} />
            </span>
            <span className="offer-card-text">
              <span className="offer-card-label">{title}</span>
              {offer.reason && <span className="offer-card-reason">{offer.reason}</span>}
            </span>
            <CornerDownLeft size={13} className="offer-card-send" aria-hidden="true" />
          </button>
        );
      })}
    </section>
  );
}
