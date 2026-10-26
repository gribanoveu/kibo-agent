import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { acceptEvent, appendUserMessage, emptyTurn, type Block } from "../lib/chatTurnReducer";
import { findingOf, reviewOf, type Finding } from "../lib/finding";
import { FindingCard, ReviewReport } from "../components/ReviewCard";
import { Transcript } from "../components/ChatPanel";

type Tool = Extract<Block, { kind: "tool" }>;

const call = (over: Partial<Tool> = {}): Tool => ({
  kind: "tool",
  id: "c1",
  round: 1,
  name: "reportFinding",
  arguments: JSON.stringify({
    path: "src/pay.rs",
    existingCode: "let fee = amount / 0;",
    title: "Divides by `zero`",
    body: "Panics on every call.",
    severity: "high",
    category: "bug",
  }),
  status: "done",
  result: { result: "findingNoted", path: "src/pay.rs", startLine: 11, endLine: 12 },
  output: "",
  ...over,
});

describe("a finding is a reportFinding call the backend kept", () => {
  test("its arguments say what, its result says where", () => {
    expect(findingOf(call())).toEqual({
      path: "src/pay.rs",
      startLine: 11,
      endLine: 12,
      severity: "high",
      title: "Divides by `zero`",
      body: "Panics on every call.",
      suggestion: undefined,
    });
  });

  /// Refused, running, or another tool: a step of the work, not a finding.
  test("a refused, unfinished or other call is none", () => {
    expect(findingOf(call({ status: "failed", result: undefined, error: "existingCode was not found" }))).toBeNull();
    expect(findingOf(call({ status: "running", result: undefined }))).toBeNull();
    expect(findingOf(call({ name: "readFile" }))).toBeNull();
    expect(findingOf(call({ arguments: "{not json" }))).toBeNull();
    expect(findingOf({ kind: "notice", id: "n", text: "x" })).toBeNull();
  });

  test("an unknown severity reads as medium", () => {
    const odd = call({ arguments: JSON.stringify({ path: "a.rs", title: "t", body: "b", severity: "urgent" }) });
    expect(findingOf(odd)?.severity).toBe("medium");
  });
});

describe("the finding card", () => {
  const finding: Finding = {
    path: "src/pay.rs",
    startLine: 11,
    endLine: 12,
    severity: "high",
    title: "Divides by `zero`",
    body: "`fee` panics.",
    suggestion: "amount / rate",
  };

  test("says what, where and why; Fix asks for it, the place opens the file", () => {
    const fixed: string[] = [];
    const opened: string[] = [];
    render(<FindingCard finding={finding} onFix={(t) => fixed.push(t)} onOpenFile={(l) => opened.push(l)} />);
    expect(screen.getByText("high")).toBeTruthy();
    expect(screen.getByText("amount / rate")).toBeTruthy();
    expect([...document.querySelectorAll(".review-finding code")].map((c) => c.textContent)).toEqual(["zero", "fee"]);
    fireEvent.click(screen.getByRole("button", { name: /Fix/ }));
    expect(fixed).toEqual(["Fix the finding from the review: Divides by `zero` (src/pay.rs:11-12)"]);
    fireEvent.click(screen.getByRole("button", { name: "src/pay.rs:11-12" }));
    expect(opened).toEqual(["src/pay.rs:11"]);
  });
});

describe("the review in the transcript", () => {
  test("a review asked to wrap up says so, with what it had spent", () => {
    const state = acceptEvent(appendUserMessage(emptyTurn(), "/review", 1000), {
      turnId: "t",
      seq: 1,
      round: 11,
      type: "wrapUpReminded",
      payload: { rounds: 10, tokens: 312_400 },
    });
    expect(state.blocks[state.blocks.length - 1]).toMatchObject({
      kind: "notice",
      text: "The review had spent 10 rounds and ~312k tokens — the agent was asked to wrap up",
    });
  });
});

describe("a review a subagent ran", () => {
  const card = (handed: boolean): Block => ({
    kind: "tool",
    id: "review",
    round: 0,
    name: "review",
    arguments: "{}",
    status: "done",
    result: {
      result: "reviewed",
      findings: [
        { path: "src/pay.rs", startLine: 11, endLine: 11, severity: "high", category: "bug", title: "Divides by zero", body: "Panics.", suggestion: null },
      ],
      text: "One real problem.",
      agent: 4,
      tokens: { prompt: 0, cached: 0, completion: 0 },
      handed,
    },
    output: "",
  });

  test("its report: the findings, what the reviewer said, whose they are", () => {
    expect(reviewOf(card(false))).toEqual({
      findings: [{ path: "src/pay.rs", startLine: 11, endLine: 11, severity: "high", category: "bug", title: "Divides by zero", body: "Panics.", suggestion: undefined } as Finding],
      summary: "One real problem.",
      agent: 4,
      handed: false,
    });
    expect(reviewOf({ ...(card(false) as Tool), status: "failed", result: undefined, error: "nothing to review" })).toBeNull();
    expect(reviewOf(call())).toBeNull();
  });

  /// The report is what the work was for: it stands in the turn, not folded into the line of work.
  test("stands on its own in the transcript", () => {
    render(<Transcript turn={{ ...emptyTurn(), status: "done", blocks: [{ kind: "user", id: "u", text: "/review" }, card(false)] }} onDecide={() => {}} />);
    expect(screen.getByText("Divides by zero")).toBeTruthy();
  });

  /// A review that failed is still the turn's one piece of work: its row stands alone, not folded away.
  test("a failed review stays in sight", () => {
    const failed = { ...(card(false) as Tool), status: "failed" as const, result: undefined, error: "the review was stopped before it finished" };
    render(<Transcript turn={{ ...emptyTurn(), status: "done", blocks: [{ kind: "user", id: "u", text: "/review" }, failed] }} onDecide={() => {}} />);
    expect(document.querySelector(".tool-run")).toBeNull();
    expect(document.querySelector(".tools")).toBeTruthy();
  });

  /// The user's findings carry Fix; the agent's do not — it answers for them itself.
  test("Fix on the user's findings, not on those handed to the agent", () => {
    const fixed: string[] = [];
    const opened: number[] = [];
    const mine = render(<ReviewReport review={reviewOf(card(false))!} onFix={(t) => fixed.push(t)} onOpenAgent={(a) => opened.push(a.id)} />);
    expect(screen.getByText("1 finding")).toBeTruthy();
    expect(screen.getByText("One real problem.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Fix/ }));
    fireEvent.click(screen.getByRole("button", { name: /Subagent/ }));
    expect(fixed).toEqual(["Fix the finding from the review: Divides by zero (src/pay.rs:11)"]);
    expect(opened).toEqual([4]);
    mine.unmount();
    render(<ReviewReport review={reviewOf(card(true))!} onFix={(t) => fixed.push(t)} />);
    expect(screen.getByText(/handed to the agent/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Fix/ })).toBeNull();
  });
});
