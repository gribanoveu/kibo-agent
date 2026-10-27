import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Transcript } from "../components/ChatPanel";
import { emptyTurn, type Block, type TurnState } from "../lib/chatTurnReducer";
import { offerOf, offersOf, type OfferKind } from "../lib/offer";

type Tool = Extract<Block, { kind: "tool" }>;

const offer = (kind: OfferKind, reason: string, over: Partial<Tool> = {}): Tool => ({
  kind: "tool",
  id: `o-${kind}-${reason}`,
  round: 1,
  name: "offer",
  arguments: JSON.stringify({ kind, reason }),
  status: "done",
  result: { result: "offered", kind },
  output: "",
  ...over,
});

const state = (blocks: Block[], over: Partial<TurnState> = {}): TurnState => ({ ...emptyTurn(), status: "done", blocks, ...over });

describe("an offer is an offer call the backend accepted", () => {
  test("its kind and reason", () => {
    expect(offerOf(offer("review", " the lock "))).toEqual({ kind: "review", reason: "the lock" });
  });

  /// Refused, unfinished, or another tool: a step of the work, not a button.
  test("a refused, unfinished or other call is none", () => {
    expect(offerOf(offer("commit", "x", { status: "failed", result: undefined, error: "nothing to commit" }))).toBeNull();
    expect(offerOf(offer("commit", "x", { status: "running", result: undefined }))).toBeNull();
    expect(offerOf(offer("commit", "x", { name: "todo" }))).toBeNull();
    expect(offerOf({ kind: "notice", id: "n", text: "x" })).toBeNull();
  });

  test("a kind offered again is one offer, with the later reason, in its first place", () => {
    const blocks = [offer("review", "first"), offer("commit", "done"), offer("review", "second")];
    expect(offersOf(blocks)).toEqual([
      { kind: "review", reason: "second" },
      { kind: "commit", reason: "done" },
    ]);
  });
});

describe("offers in the transcript", () => {
  const turn: Block[] = [
    { kind: "user", id: "u1", text: "make the counter race-free" },
    offer("review", "the lock in two modules"),
    { kind: "message", id: "m1", round: 2, text: "Done: the counter takes the lock." },
  ];

  test("drawn after the answer, not where the call was; a press sends the kind", () => {
    const picked: OfferKind[] = [];
    render(<Transcript turn={state(turn)} onDecide={() => {}} onOffer={(kind) => picked.push(kind)} />);
    const answer = screen.getByText("Done: the counter takes the lock.");
    const card = screen.getByRole("button", { name: /Run a subagent for review/ });
    expect(answer.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("the lock in two modules")).toBeTruthy();
    // Not a step of the folded work either.
    expect(screen.queryByText(/Offer/)).toBeNull();
    fireEvent.click(card);
    expect(picked).toEqual(["review"]);
  });

  /// A notice after the answer — a hook, a cache miss — is not the end of the turn's offers.
  test("still there after a notice that follows the answer", () => {
    render(<Transcript turn={state([...turn, { kind: "notice", id: "n1", text: "Cache missed" }])} onDecide={() => {}} onOffer={() => {}} />);
    expect(screen.getByRole("button", { name: /Run a subagent for review/ })).toBeTruthy();
  });

  test("none while the turn runs, after the user's next message, or with nowhere to send them", () => {
    const { unmount } = render(<Transcript turn={state(turn, { status: "running" })} onDecide={() => {}} onOffer={() => {}} />);
    expect(screen.queryByText("Run a subagent for review")).toBeNull();
    unmount();
    const next: Block[] = [...turn, { kind: "user", id: "u2", text: "/review" }];
    const again = render(<Transcript turn={state(next)} onDecide={() => {}} onOffer={() => {}} />);
    expect(screen.queryByText("Run a subagent for review")).toBeNull();
    again.unmount();
    render(<Transcript turn={state(turn)} onDecide={() => {}} />);
    expect(screen.queryByText("Run a subagent for review")).toBeNull();
  });
});
