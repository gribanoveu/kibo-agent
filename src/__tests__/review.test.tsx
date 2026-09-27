import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { acceptEvent, appendUserMessage, emptyTurn, reviewEnded, reviewStarted, type Block } from "../lib/chatTurnReducer";
import { ReviewCard } from "../components/ReviewCard";
import type { Finding, GroupProgress, ReviewReport } from "../lib/chat";
import { groupName } from "../components/ReviewCard";

const finding = (over: Partial<Finding> = {}): Finding => ({
  id: 1,
  path: "src/pay.rs",
  startLine: 11,
  endLine: 11,
  severity: "high",
  category: "bug",
  title: "Divides by zero",
  body: "Panics on every call.",
  ...over,
});

const report = (over: Partial<ReviewReport> = {}): ReviewReport => ({
  reviewed: ["src/pay.rs", "src/b.rs"],
  excluded: [],
  findings: [],
  failed: [],
  summaries: [],
  ...over,
});

const progress = (over: Partial<GroupProgress> = {}): GroupProgress => ({
  group: 0,
  total: 2,
  files: ["src/pay.rs"],
  state: "waiting",
  failedCalls: 0,
  findings: 0,
  input: 0,
  output: 0,
  truncated: false,
  estimate: 0,
  ...over,
});

type Review = Extract<Block, { kind: "review" }>;
const reviewBlock = (blocks: Block[]) => blocks.find((b): b is Review => b.kind === "review");

describe("the review in the transcript", () => {
  test("a card under way takes each group's progress, then its report, and the turn ends", () => {
    let state = reviewStarted(appendUserMessage(emptyTurn(), "/review", 1000));
    expect(state.status).toBe("running");
    expect(reviewBlock(state.blocks)).toMatchObject({ groups: [], report: null });

    const event = (seq: number, payload: GroupProgress) => ({ turnId: "t", seq, round: 0, type: "reviewGroup" as const, payload });
    state = acceptEvent(state, event(1, progress({ group: 1, files: ["b.rs"] })));
    state = acceptEvent(state, event(2, progress({ group: 0 })));
    state = acceptEvent(state, event(3, progress({ group: 1, files: ["b.rs"], state: "working" })));
    // A group's line is replaced, not added to, and the lines keep their order.
    expect(reviewBlock(state.blocks)?.groups.map((g) => [g.group, g.state])).toEqual([
      [0, "waiting"],
      [1, "working"],
    ]);

    state = reviewEnded(state, report({ findings: [finding()] }), 2000);
    expect(state.status).toBe("done");
    expect(reviewBlock(state.blocks)?.report?.findings).toHaveLength(1);
    // The clock endTurn stops is kept on the bubble: "Worked for" under a review too.
    expect(state.blocks.find((b) => b.kind === "user")).toMatchObject({ workedMs: 1000 });
    // A finished card is not moved by a late event.
    const late = acceptEvent(state, event(4, progress({ group: 0, state: "done" })));
    expect(reviewBlock(late.blocks)?.groups[0].state).toBe("waiting");
  });
});

describe("the review card", () => {
  const card = (block: Partial<Review>, handlers: { onFix?: (t: string) => void; onOpenFile?: (l: string) => void } = {}) =>
    render(<ReviewCard block={{ kind: "review", id: "r", groups: [], report: null, ...block }} {...handlers} />);

  test("before the groups are known it says it is reading the changes", () => {
    card({});
    expect(screen.getByRole("status").textContent).toContain("Reading the changes…");
  });

  test("says what each group is doing, what it cost, and what is slowing it", () => {
    card({
      groups: [
        progress({ state: "done", findings: 2, failedCalls: 1, input: 30_000, output: 18_000 }),
        progress({
          group: 1,
          files: ["src/lib/a.ts", "src/lib/b.ts", "src/lib/deep/c.ts"],
          state: "working",
          note: "No answer from the provider — retry 2 of 5 in 30s",
        }),
      ],
    });
    const text = screen.getByRole("status").textContent ?? "";
    expect(text).toContain("1 of 2 groups · 48k tokens");
    expect(text).toContain("src/lib · 3 files");
    expect(text).toContain("Reviewing…");
    expect(text).toContain("retry 2 of 5 in 30s");
    expect(text).toContain("2 findings");
    // In and out apart: the reply's thinking can outweigh the diff.
    expect(text).toContain("30k in · 18k out · 1 not placed");
  });

  test("a group is named by the folder its files share", () => {
    expect(groupName(["src/a.rs"])).toBe("src/a.rs");
    expect(groupName(["src/x/a.rs", "src/y/b.rs"])).toBe("src · 2 files");
    expect(groupName(["a.rs", "src/b.rs"])).toBe("2 files");
  });

  test("lists each finding with its place, and Fix asks for it by number", () => {
    const fixed: string[] = [];
    const opened: string[] = [];
    card(
      { report: report({ findings: [finding(), finding({ id: 2, severity: "low", title: "Slow", startLine: 3, endLine: 5, suggestion: "cache it" })] }) },
      { onFix: (t) => fixed.push(t), onOpenFile: (l) => opened.push(l) },
    );
    expect(screen.getByText("2 findings")).toBeTruthy();
    expect(screen.getByText("2 files reviewed")).toBeTruthy();
    expect(screen.getByText("cache it")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: /Fix/ })[1]);
    expect(fixed).toEqual(["Fix finding 2 from the review: Slow (src/pay.rs:3-5)"]);
    fireEvent.click(screen.getByRole("button", { name: "src/pay.rs:11" }));
    expect(opened).toEqual(["src/pay.rs:11"]);
  });

  test("with nothing found says so; what was left out is shown on asking", () => {
    card({ report: report({ excluded: [{ path: "Cargo.lock", reason: "generated" }] }) });
    expect(screen.getByText("No problems found")).toBeTruthy();
    expect(screen.queryByText(/lockfile/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "1 left out" }));
    expect(screen.getByText(/generated or a lockfile/)).toBeTruthy();
  });

  test("a finished review keeps its groups, shown on asking with what they cost", () => {
    card({
      groups: [progress({ state: "done", findings: 1, input: 30_000, output: 10_000 }), progress({ group: 1, files: ["b.rs"], state: "done", input: 2_000 })],
      report: report(),
    });
    expect(screen.queryByText("30k in · 10k out")).toBeNull();
    expect(screen.getByText(/42k tokens/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "2 groups" }));
    expect(screen.getByText("30k in · 10k out")).toBeTruthy();
  });

  test("with nothing found, the summary says what the change does and what was checked", () => {
    const opened: string[] = [];
    card(
      {
        report: report({
          summaries: [
            {
              files: ["src/pay.rs"],
              summary: "Adds a fee to every payment.",
              checked: ["rounding", "zero amounts"],
              worthALook: [{ path: "src/api.rs", note: "callers may pass a negative amount" }],
            },
          ],
        }),
      },
      { onOpenFile: (l) => opened.push(l) },
    );
    expect(screen.getByText("No problems found")).toBeTruthy();
    // Its own panel: the group's files, and how many findings are in them.
    expect(screen.getByText("src/pay.rs")).toBeTruthy();
    expect(screen.getByText("No findings")).toBeTruthy();
    expect(screen.getByText("Adds a fee to every payment.")).toBeTruthy();
    // What was checked runs long: folded under a count, listed when opened.
    const checked = screen.getByText("Checked 2").closest("details") as HTMLDetailsElement;
    expect(checked.open).toBe(false);
    expect([...checked.querySelectorAll("li")].map((li) => li.textContent)).toEqual(["rounding", "zero amounts"]);
    expect(screen.getByText(/callers may pass a negative amount/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "src/api.rs" }));
    expect(opened).toEqual(["src/api.rs"]);
  });

  test("findings come first, and each group's panel counts the ones in its files", () => {
    card({
      report: report({
        findings: [finding(), finding({ id: 2, path: "src/pay.rs", startLine: 20, endLine: 20 })],
        summaries: [
          { files: ["src/pay.rs"], summary: "Adds a fee.", checked: [], worthALook: [] },
          { files: ["src/b.rs"], summary: "Renames a field.", checked: [], worthALook: [] },
        ],
      }),
    });
    const sections = [...document.querySelectorAll(".review-section")].map((s) => s.textContent);
    expect(sections).toEqual(["Findings", "By group"]);
    expect(screen.getByText("2 findings", { selector: ".review-chip" })).toBeTruthy();
    expect(screen.getByText("No findings", { selector: ".review-chip" })).toBeTruthy();
  });

  /// Why a group has no summary, when the reason is known: its reply was cut.
  test("a reply cut at the length limit is said, on its line and where its summary would be", () => {
    card({
      groups: [progress({ files: ["src/pay.rs"], state: "done", truncated: true, input: 40_000, output: 64_000 })],
      report: report({}),
    });
    expect(screen.getByText(/cut off at the provider's length limit before it came/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "1 group" }));
    expect(screen.getByText("Cut off at the provider's length limit")).toBeTruthy();
  });

  test("a finished group without a summary is named as having none", () => {
    card({
      groups: [
        progress({ files: ["src/pay.rs"], state: "done" }),
        progress({ group: 1, files: ["src-tauri/a.rs", "src-tauri/b.rs"], state: "done" }),
        progress({ group: 2, files: ["web/x.ts"], state: "failed", error: "provider said 500" }),
      ],
      report: report({ summaries: [{ files: ["src/pay.rs"], summary: "Adds a fee.", checked: [], worthALook: [] }] }),
    });
    expect(screen.getByText("Adds a fee.")).toBeTruthy();
    // Two summaries' worth of groups: each is named, the silent one too.
    expect(screen.getByText("src/pay.rs")).toBeTruthy();
    expect(screen.getByText("src-tauri · 2 files")).toBeTruthy();
    expect(screen.getAllByText("No summary — the model did not send one for these files.")).toHaveLength(1);
  });

  test("before the provider counts, the request's own estimate is shown, marked", () => {
    card({ groups: [progress({ state: "working", estimate: 38_000 }), progress({ group: 1, state: "done", input: 15_000, output: 5_000, estimate: 19_000 })] });
    const text = screen.getByRole("status").textContent ?? "";
    expect(text).toContain("1 of 2 groups · ~58k tokens");
    expect(text).toContain("~38k in");
  });

  test("code the model quotes in backticks is drawn as code", () => {
    card({ report: report({ findings: [finding({ body: "`send` drops `sent` here." })] }) });
    const codes = [...document.querySelectorAll(".review-body code")].map((c) => c.textContent);
    expect(codes).toEqual(["send", "sent"]);
    expect(screen.getByText(/drops/).textContent).toBe("send drops sent here.");
  });

  test("a clean tree has nothing to review", () => {
    card({ report: report({ reviewed: [] }) });
    expect(screen.getByText("Nothing to review")).toBeTruthy();
  });

  test("a group that failed is named", () => {
    card({ report: report({ failed: [{ files: ["a.rs", "b.rs"], error: "provider said 429" }] }) });
    expect(screen.getByText("Not finished — provider said 429: a.rs, b.rs")).toBeTruthy();
  });
});
