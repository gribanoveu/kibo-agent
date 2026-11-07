import { CornerDownLeft, GitCommitHorizontal, GitFork, GitPullRequest, ListChecks, SearchCheck } from "lucide-react";
import type { Offer, OfferKind } from "../lib/offer";
import "./OfferCards.css";

const LOOK: Record<OfferKind, { label: string; icon: typeof SearchCheck }> = {
  review: { label: "Run a subagent for review", icon: SearchCheck },
  commit: { label: "Commit", icon: GitCommitHorizontal },
  worktree: { label: "Move this chat to a worktree", icon: GitFork },
  pullRequest: { label: "Create a pull request", icon: GitPullRequest },
  implement: { label: "Implement the plan", icon: ListChecks },
};

/** The next steps the agent offered, after its answer: each a button that sends it as the user's message. */
export function OfferCards({
  offers,
  onPick,
  onOpenPlan,
}: {
  offers: Offer[];
  onPick: (kind: OfferKind) => void;
  /** Opens the Plan tab: a plan is read before it is implemented. */
  onOpenPlan?: () => void;
}) {
  if (offers.length === 0) return null;
  return (
    <section className="offer-cards" aria-label="Suggested next steps">
      <span className="offer-cards-caption" aria-hidden="true">
        {offers.length === 1 ? "Suggested next step" : "Suggested next steps"}
      </span>
      {offers.map((offer) => {
        const { label, icon: Icon } = LOOK[offer.kind];
        return (
          <button key={offer.kind} type="button" className={`offer-card ${offer.kind}`} onClick={() => onPick(offer.kind)}>
            <span className="offer-card-icon">
              <Icon size={14} />
            </span>
            <span className="offer-card-text">
              <span className="offer-card-label">{label}</span>
              {offer.reason && <span className="offer-card-reason">{offer.reason}</span>}
            </span>
            <CornerDownLeft size={13} className="offer-card-send" aria-hidden="true" />
          </button>
        );
      })}
      {onOpenPlan && offers.some((offer) => offer.kind === "implement") && (
        <button type="button" className="btn btn-ghost offer-cards-plan" onClick={onOpenPlan}>
          Review the plan
        </button>
      )}
    </section>
  );
}
