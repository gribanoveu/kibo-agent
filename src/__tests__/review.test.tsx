import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { acceptEvent, appendUserMessage, emptyTurn, type Block } from "../lib/chatTurnReducer";
import { findingOf, type Finding } from "../lib/finding";
import { FindingCard } from "../components/ReviewCard";

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
