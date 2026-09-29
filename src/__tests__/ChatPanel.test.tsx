import { describe, expect, test } from "bun:test";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { ChatPanel, Preview, Transcript, formatDuration } from "../components/ChatPanel";
import { emptyTurn, type Block, type TurnState } from "../lib/chatTurnReducer";

// The transcript's own rules: who a block belongs to, and what the approval
// card sends back. Both are decided here rather than by the backend, so both
// are checked here.

const state = (blocks: Block[], over: Partial<TurnState> = {}): TurnState => ({
  ...emptyTurn(),
  blocks,
  ...over,
});

const panel = (
  turn: TurnState,
  onDecide = () => {},
  over: { onImplement?: () => void } = {},
) =>
  render(
    <ChatPanel
      workspace="/tmp/project"
      turn={turn}
      onDecide={onDecide}
      onOpenRepo={() => {}}
      onImplement={over.onImplement}
    />,
  );

describe("handing a plan to Agent mode", () => {
  const answered = [
    { kind: "user", id: "u0", text: "plan the fix" },
    { kind: "message", id: "m1", round: 1, text: "1. read 2. fix" },
  ] as Block[];

  test("offered under a finished answer, and pressing it hands the plan over", () => {
    let handed = 0;
    panel(state(answered, { status: "done" }), undefined, { onImplement: () => handed++ });
    fireEvent.click(screen.getByText("Implement in Agent mode"));
    expect(handed).toBe(1);
  });

  test("not while the plan is being written, waiting, stopped, or outside Plan mode", () => {
    for (const status of ["running", "awaitingApproval", "cancelled"] as const) {
      const { unmount } = panel(state(answered, { status }), undefined, { onImplement: () => {} });
      expect(screen.queryByText("Implement in Agent mode")).toBeNull();
      unmount();
    }
    panel(state(answered, { status: "done" }));
    expect(screen.queryByText("Implement in Agent mode")).toBeNull();
  });

  test("not when the last thing shown is not the plan — a compaction note, say", () => {
    const noted = [...answered, { kind: "notice", id: "n1", text: "Earlier messages were summarized." }] as Block[];
    panel(state(noted, { status: "done" }), undefined, { onImplement: () => {} });
    expect(screen.queryByText("Implement in Agent mode")).toBeNull();
  });
});

describe("grouping", () => {
  test("everything after a question belongs to the answer", () => {
    panel(
      state([
        { kind: "user", id: "u0", text: "fix it" },
        { kind: "message", id: "m1", round: 1, text: "looking" },
        { kind: "tool", id: "c1", round: 1, name: "readFile", arguments: '{"path":"a.rs"}', status: "done", output: "" },
        { kind: "message", id: "m2", round: 2, text: "fixed" },
      ]),
    );

    // One "You" and one "Agent" label: the tool call did not open a third turn.
    expect(screen.getAllByText("You")).toHaveLength(1);
    expect(screen.getAllByText("Agent")).toHaveLength(1);
  });

  test("a second question opens a second turn", () => {
    panel(
      state([
        { kind: "user", id: "u0", text: "first" },
        { kind: "message", id: "m1", round: 1, text: "done" },
        { kind: "user", id: "u1", text: "second" },
      ]),
    );

    expect(screen.getAllByText("You")).toHaveLength(2);
  });

  test("with nothing said yet the empty state fills the thread", () => {
    panel(state([]));
    expect(screen.getByText("Start the conversation")).toBeDefined();
    // It already is a new chat: offering another one does nothing.
    expect(screen.queryByRole("button", { name: /New chat/ })).toBeNull();
  });
});

describe("a tool row", () => {
  const read: Block = {
    kind: "tool",
    id: "c1",
    round: 1,
    name: "readFile",
    arguments: '{"path":"TaxService.java"}',
    status: "done",
    result: { content: "119  income = null;", startLine: 119, endLine: 124 },
    output: "",
  };

  test("shows what it did without being expanded", () => {
    panel(state([read]));

    expect(screen.getByText("Read")).toBeDefined();
    expect(screen.getByText("TaxService.java")).toBeDefined();
    expect(screen.getByText("lines 119-124")).toBeDefined();
    expect(screen.queryByText(/income = null/)).toBeNull();
  });

  test("and the body only when asked", () => {
    panel(state([read]));

    fireEvent.click(screen.getByText("Read"));
    expect(screen.getByText(/income = null/)).toBeDefined();
  });
});

describe("a run of calls", () => {
  const call = (id: string, name: string, status: "done" | "running" = "done"): Block => ({
    kind: "tool",
    id,
    round: 1,
    name,
    arguments: '{"path":"a.rs"}',
    status,
    output: "",
  });

  test("folds into one line, and a message between two runs keeps them apart", () => {
    const { container } = panel(
      state([
        call("c1", "readFile"),
        { kind: "reasoning", id: "r1", round: 1, text: "now grep" },
        call("c2", "grep"),
        { kind: "message", id: "m1", round: 1, text: "found it" },
        call("c3", "runCommand"),
      ] as Block[]),
    );
    const runs = [...container.querySelectorAll(".tool-run > summary")].map((s) => s.textContent);
    expect(runs).toEqual(["Read a file, searched a pattern", "Ran a command"]);
  });

  test("names the call under way while the turn runs", () => {
    const { container } = panel(state([call("c1", "readFile"), call("c2", "readFile", "running")], { status: "running" }));
    expect(container.querySelector(".tool-run-step")?.textContent).toBe("Reading a.rs…");
    expect(container.querySelector(".tool-run-count")?.textContent).toBe("2 calls");
  });

  test("a run the agent has written past is not under way", () => {
    const { container } = panel(
      state(
        [call("c1", "readFile"), { kind: "message", id: "m1", round: 1, text: "now" } as Block, call("c2", "grep", "running")],
        { status: "running" },
      ),
    );
    const live = [...container.querySelectorAll(".tool-run")].map((r) => r.classList.contains("live"));
    expect(live).toEqual([false, true]);
  });
});

/** The card asks the backend what its calls would do; let that answer land. */
const settle = () => act(async () => {});

describe("the approval card", () => {
  const approval: Block = {
    kind: "approval",
    id: "approval:1",
    round: 1,
    calls: [
      { id: "w1", name: "writeFile", arguments: '{"path":"a.rs"}', requiresConfirmation: true },
      { id: "l1", name: "listFiles", arguments: "{}", requiresConfirmation: false },
    ],
  };

  test("asks only about the calls that need an answer", async () => {
    panel(state([approval]));
    await settle();

    expect(screen.getByText(/Approval required/).textContent).toContain("Write");
    expect(screen.getByText(/Approval required/).textContent).not.toContain("List");
  });

  test("allowing answers every call that was asked about", async () => {
    const decided: unknown[] = [];
    panel(state([approval]), (decisions, always) => decided.push({ decisions, always }));
    await settle();

    fireEvent.click(screen.getByText("Allow"));

    expect(decided).toEqual([
      { decisions: [{ id: "w1", approved: true, reason: null }], always: [] },
    ]);
  });

  /// A model told only "denied" tries the same call again, then a near variant
  /// of it. The reason is what ends that in one round — so it has to reach the
  /// decision, not just the textbox.
  test("a refusal carries the reason that was typed", async () => {
    const decided: { decisions: { reason?: string | null }[] }[] = [];
    panel(state([approval]), (decisions) => decided.push({ decisions }));
    await settle();

    fireEvent.change(screen.getByPlaceholderText(/Why not/), {
      target: { value: "use the existing helper" },
    });
    fireEvent.click(screen.getByText("Deny"));

    expect(decided[0]?.decisions[0]?.reason).toBe("use the existing helper");
  });

  test("calls that need no answer fold into one line, counted", async () => {
    panel(
      state([
        {
          ...approval,
          calls: [
            { id: "w1", name: "writeFile", arguments: '{"path":"a.rs"}', requiresConfirmation: true },
            { id: "t1", name: "todo", arguments: '{"op":"update"}', requiresConfirmation: false },
            { id: "t2", name: "todo", arguments: '{"op":"update"}', requiresConfirmation: false },
            { id: "l1", name: "listFiles", arguments: "{}", requiresConfirmation: false },
          ],
        } as Block,
      ]),
    );
    await settle();

    const passive = document.querySelectorAll(".approval-cmd.passive");
    expect(passive).toHaveLength(1);
    expect(passive[0].textContent).toMatch(/×2, List/);
    expect(screen.queryByText("update")).toBeNull();
  });

  test("always allow names the tool it should stop asking about", async () => {
    const decided: { always: string[] }[] = [];
    panel(state([approval]), (_decisions, always) => decided.push({ always }));
    await settle();

    fireEvent.click(screen.getByText("Always"));

    expect(decided[0]?.always).toEqual(["writeFile"]);
  });
});

describe("what a call would do", () => {
  test("a write shows its diff, coloured line by line", () => {
    render(
      <Preview
        preview={{
          kind: "diff",
          path: "Mapper.java",
          diff: {
            linesAdded: 1,
            linesRemoved: 1,
            unifiedDiff: "@@ -1 +1 @@\n-return null;\n+return IncomeAmount.zero();\n",
            truncated: false,
          },
        }}
      />,
    );

    expect(screen.getByText("Mapper.java")).toBeDefined();
    expect(screen.getByText("+1")).toBeDefined();
    expect(document.querySelectorAll(".diff-add")).toHaveLength(1);
    expect(document.querySelectorAll(".diff-del")).toHaveLength(1);
    // Of the edited line, only what changed is marked.
    expect([...document.querySelectorAll(".diff-view mark")].map((m) => m.textContent)).toEqual(["null", "IncomeAmount.zero()"]);
  });

  /// Learning that an edit cannot apply after approving it costs a round and
  /// the user's trust in the card.
  test("an edit that cannot apply says so instead", () => {
    render(<Preview preview={{ kind: "failed", reason: "edit text not found: nowhere" }} />);

    expect(screen.getByText(/would not succeed/).textContent).toContain("not found");
  });

  /// The scariest thing to approve is a path that turned out broader than it
  /// looked.
  test("a recursive delete says how much it would take", () => {
    render(<Preview preview={{ kind: "removes", path: "src", files: 428 }} />);
    expect(screen.getByText(/428 files/)).toBeDefined();
  });

  test("a command says where it would run", () => {
    render(<Preview preview={{ kind: "command", command: "cargo test", cwd: "crate" }} />);
    expect(screen.getByText(/Runs in crate/)).toBeDefined();
  });

  test("a call with nothing to show draws nothing at all", () => {
    const { container } = render(<Preview preview={{ kind: "nothing" }} />);
    expect(container.textContent).toBe("");
  });

  test("and so does a call whose preview has not arrived yet", () => {
    const { container } = render(<Preview preview={undefined} />);
    expect(container.textContent).toBe("");
  });
});

describe("how long the agent worked", () => {
  const turn: Block[] = [
    { kind: "user", id: "u0", text: "fix it", workedMs: 83_000 },
    { kind: "message", id: "m1", round: 1, text: "fixed" },
  ] as Block[];

  test("reads like Claude Code's", () => {
    expect([formatDuration(12_400), formatDuration(83_000), formatDuration(3_900_000)]).toEqual([
      "12s",
      "1m 23s",
      "1h 5m",
    ]);
  });

  test("is said under a finished answer", () => {
    panel(state(turn, { status: "done" }));
    expect(screen.getByText("Worked for 1m 23s")).toBeTruthy();
  });

  test("ticks at the end of the thread while the turn runs", () => {
    panel(state([{ kind: "user", id: "u0", text: "fix it" }], { status: "running", runningSince: Date.now() - 5_000 }));
    expect(screen.getByRole("timer").textContent).toBe("Working… 5s");
    expect(screen.queryByText(/Worked for/)).toBeNull();
  });

  /// Background processes outlive the turn: counted beside the clock, and a
  /// click opens the newest.
  test("counts the background processes still running, and opens the newest", () => {
    const opened: number[] = [];
    const running = (turnState: TurnState, ids: number[]) =>
      render(<ChatPanel workspace="/tmp/p" turn={turnState} onDecide={() => {}} onOpenRepo={() => {}} runningProcesses={ids} onOpenProcess={(id) => opened.push(id)} />);

    const live = running(state([{ kind: "user", id: "u0", text: "fix it" }], { status: "running", runningSince: Date.now() - 5_000 }), [3]);
    expect(screen.getByRole("timer").textContent).toBe("Working… 5s·1 process running");
    live.unmount();

    running(state(turn, { status: "done" }), [4, 2]);
    fireEvent.click(screen.getByRole("button", { name: "2 processes running" }));
    expect(opened).toEqual([4]);
  });

  test("says nothing of processes when none runs", () => {
    panel(state(turn, { status: "done" }));
    expect(screen.queryByText(/running/)).toBeNull();
  });
});

describe("the header", () => {
  /// The folder, its branch and its changes are on the composer's tab now.
  test("names the chat, and only the chat", () => {
    render(
      <ChatPanel
        title="Fix the tax rounding"
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
      />,
    );
    expect(screen.getByRole("heading", { name: "Fix the tax rounding" })).toBeTruthy();
    expect(screen.queryByTitle("/tmp/project")).toBeNull();
  });

  test("shows and hides the side panel, and says which it would do", () => {
    let toggled = 0;
    const { rerender } = render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onToggleAside={() => toggled++}
      />,
    );
    fireEvent.click(screen.getByTitle(/^Show changes/));
    expect(toggled).toBe(1);

    rerender(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        asideOpen
        onToggleAside={() => toggled++}
      />,
    );
    expect(screen.getByTitle(/^Hide changes/).getAttribute("aria-pressed")).toBe("true");
  });

  test("the … menu opens a side panel by name", () => {
    const opened: string[] = [];
    render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onOpenPanel={(tab) => opened.push(tab)}
      />,
    );
    fireEvent.click(screen.getByTitle("More"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Terminal" }));

    expect(opened).toEqual(["terminal"]);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  test("Terminal has its own header button, and leaves the … menu", () => {
    let toggled = 0;
    render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onOpenPanel={() => {}}
        terminalOpen
        onToggleTerminal={() => toggled++}
      />,
    );
    const button = screen.getByTitle(/^Hide terminal/);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(button);
    expect(toggled).toBe(1);

    fireEvent.click(screen.getByTitle("More"));
    expect(screen.queryByRole("menuitem", { name: "Terminal" })).toBeNull();
  });

  test("the … menu offers the export, and closes once it is picked", () => {
    let exported = 0;
    render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onExport={() => exported++}
      />,
    );
    expect(screen.queryByRole("menu")).toBeNull();

    fireEvent.click(screen.getByTitle("More"));
    fireEvent.click(screen.getByRole("menuitem", { name: /Export as Markdown/ }));

    expect(exported).toBe(1);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  test("the … menu closes on Escape, and is not there with nothing in it", () => {
    const { unmount } = render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onExport={() => {}}
      />,
    );
    fireEvent.click(screen.getByTitle("More"));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    unmount();

    render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
      />,
    );
    expect(screen.queryByTitle("More")).toBeNull();
  });

  test("a chat not saved yet is a new one", () => {
    panel(state([]));
    expect(screen.getByRole("heading", { name: "New chat" })).toBeTruthy();
  });
});

describe("a notice", () => {
  /// It is the app talking, not the agent. Under an "Agent" label it reads as
  /// something the model said about itself.
  test("belongs to neither side of the conversation", () => {
    panel(
      state([
        { kind: "user", id: "u0", text: "hi" },
        { kind: "notice", id: "n0", text: "Older history compacted" },
      ]),
    );

    expect(screen.getByText("Older history compacted")).toBeTruthy();
    expect(screen.queryByText("Agent")).toBeNull();
  });
});

describe("a compaction", () => {
  const card = (block: Extract<Block, { kind: "compaction" }>) => {
    panel(state([block]));
    return screen.getByRole("status").textContent;
  };

  test("is a card while the summary is made", () => {
    expect(card({ kind: "compaction", id: "c0", status: "running" })).toContain("Compacting history");
    expect(screen.queryByText("Agent")).toBeNull();
  });

  test("says what was folded", () => {
    expect(card({ kind: "compaction", id: "c0", status: "done", folded: 1 })).toContain("1 message folded");
  });

  test("says when it gave up", () => {
    expect(card({ kind: "compaction", id: "c0", status: "failed" })).toContain("not compacted");
  });
});

describe("copying a message", () => {
  const blocks = [
    { kind: "user", id: "u0", text: "why **bold**?" },
    { kind: "message", id: "m1", round: 1, text: "Because `**` is **Markdown**." },
  ] as Block[];

  test("copies what was written — the answer's Markdown source, not the page", async () => {
    const copied: string[] = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (text: string) => void copied.push(text) },
    });
    panel(state(blocks, { status: "done" }));

    const [question, answer] = screen.getAllByRole("button", { name: "Copy" });
    await act(async () => fireEvent.click(question));
    await act(async () => fireEvent.click(answer));
    expect(copied).toEqual(["why **bold**?", "Because `**` is **Markdown**."]);
    expect(screen.getAllByRole("button", { name: "Copied" })).toHaveLength(2);
  });

  test("the answer being written has nothing to copy yet", () => {
    panel(state(blocks, { status: "running" }));
    expect(screen.getAllByRole("button", { name: "Copy" })).toHaveLength(1);
  });
});

describe("branching from a message", () => {
  const blocks: Block[] = [
    { kind: "user", id: "u0", text: "folded away" },
    { kind: "user", id: "u1", text: "still seen" },
  ];
  const branchPanel = (branchable: ReadonlySet<string> | null, onBranch: (id: string) => void = () => {}) =>
    render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state(blocks, { status: "done" })}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        branchable={branchable}
        onBranch={onBranch}
      />,
    );

  test("is offered where the model still sees the message, and says why not elsewhere", () => {
    const picked: string[] = [];
    branchPanel(new Set(["u1"]), (id) => picked.push(id));

    const [folded, seen] = screen.getAllByRole("button", { name: "Branch from here" }) as HTMLButtonElement[];
    expect(folded.disabled).toBe(true);
    expect(folded.title).toContain("Folded into the summary");
    expect(seen.disabled).toBe(false);

    fireEvent.click(seen);
    expect(picked).toEqual(["u1"]);
  });

  test("is not offered while a turn is under way", () => {
    branchPanel(null);
    expect(screen.queryAllByRole("button", { name: "Branch from here" })).toHaveLength(0);
  });
});

describe("a card that asks although its tool is always allowed", () => {
  test("says why, and an ordinary card says nothing extra", () => {
    const block: Block = {
      kind: "approval",
      id: "approval:2",
      round: 2,
      calls: [
        {
          id: "r1",
          name: "runCommand",
          arguments: '{"command":"git pf"}',
          requiresConfirmation: true,
          reason: "rewrites a remote (git push --force)",
        },
        { id: "r2", name: "runCommand", arguments: '{"command":"cargo test"}', requiresConfirmation: true },
      ],
    };
    panel(state([block], { status: "awaitingApproval" }));

    const why = screen.getAllByText(/Always asks:/);
    expect(why).toHaveLength(1);
    expect(why[0].textContent).toBe("Always asks: rewrites a remote (git push --force)");
  });
});

describe("the answer as Markdown", () => {
  // The repair of half-written markup drops everything after an unmatched
  // `![`: right for the answer still arriving, content loss for any other.
  const blocks = [
    { kind: "user", id: "u0", text: "go" },
    { kind: "message", id: "m1", round: 1, text: "Earlier ![one and the tail" },
    { kind: "message", id: "m2", round: 2, text: "Now ![two and more" },
  ] as Block[];

  test("only the last block of a running turn is repaired as it streams", () => {
    const { container } = panel(state(blocks, { status: "running" }));
    expect(container.textContent).toContain("and the tail");
    expect(container.textContent).not.toContain("and more");
  });

  test("once the turn ends, nothing is", () => {
    const { container } = panel(state(blocks, { status: "done" }));
    expect(container.textContent).toContain("and more");
  });
});

describe("a finished call's diff", () => {
  test("opens as a diff, not as text", () => {
    const { container } = panel(
      state([
        {
          kind: "tool",
          id: "d1",
          round: 1,
          name: "gitDiff",
          arguments: '{"path":"a.md"}',
          status: "done",
          output: "",
          result: { path: "a.md", label: "index → working tree", isBinary: false, diff: { linesAdded: 1, linesRemoved: 1, unifiedDiff: "@@ -1 +1 @@\n-one\n+uno\n", truncated: false } },
        } as Block,
      ]),
    );
    fireEvent.click(container.querySelector(".tool-item button.tool")!);

    expect(container.querySelector(".tool-detail")).toBeNull();
    expect(container.querySelectorAll(".diff-view .diff-add")).toHaveLength(1);
  });
});

describe("a background process started from the chat", () => {
  const started = {
    kind: "tool",
    id: "b1",
    round: 1,
    name: "runCommand",
    arguments: '{"command":"npm run dev","background":true}',
    status: "done",
    result: { result: "processStarted", id: 3, command: "npm run dev", cwd: ".", state: { state: "running" } },
    output: "",
  } as Block;

  test("its row opens it in the Terminal tab, by id", () => {
    const opened: number[] = [];
    render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([started])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onOpenProcess={(id) => opened.push(id)}
      />,
    );
    const row = screen.getByTitle("Show in Terminal") as HTMLButtonElement;
    expect(row.disabled).toBe(false);
    fireEvent.click(row);
    expect(opened).toEqual([3]);
  });

  test("with nowhere to open it, the row stays as it was", () => {
    panel(state([started]));
    expect(screen.queryByTitle("Show in Terminal")).toBeNull();
  });
});

describe("a helper's explore row", () => {
  const explored = {
    kind: "tool",
    id: "e1",
    round: 1,
    name: "explore",
    arguments: '{"task":"Where is X?"}',
    status: "done",
    result: { text: "X is in a.rs:1", agent: 4, tokens: { prompt: 1000, cached: 0, completion: 10 } },
    output: "grep X\n",
  } as Block;

  test("unfolds to its answer, with its run in the Agents tab a click away", () => {
    const opened: unknown[] = [];
    render(
      <ChatPanel workspace="/tmp/project" turn={state([explored])} onDecide={() => {}} onOpenRepo={() => {}} onOpenAgent={(agent) => opened.push(agent)} />,
    );
    expect(screen.queryByText("Show in Agents")).toBeNull();
    fireEvent.click(screen.getByText("Where is X?"));
    expect(screen.getByText("X is in a.rs:1")).toBeTruthy();
    fireEvent.click(screen.getByText("Show in Agents"));
    expect(opened).toEqual([{ id: 4, task: "Where is X?" }]);
  });

  test("with nowhere to open it, the answer is all there is", () => {
    panel(state([explored]));
    fireEvent.click(screen.getByText("Where is X?"));
    expect(screen.getByText("X is in a.rs:1")).toBeTruthy();
    expect(screen.queryByText("Show in Agents")).toBeNull();
  });
});

describe("a background process that ended", () => {
  const ended = (code: number) =>
    ({
      kind: "processEnded",
      id: "ended:0",
      process: { id: 4, command: "cargo build", cwd: "src-tauri", state: { state: "exited", code } },
    }) as Block;

  test("is a card with the command, the folder and how it ended, that opens the Terminal tab", () => {
    const opened: number[] = [];
    const { container } = render(
      <ChatPanel
        workspace="/tmp/project"
        turn={state([ended(101)])}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        onOpenProcess={(id) => opened.push(id)}
      />,
    );
    const card = screen.getByTitle("Show in Terminal");
    expect(card.className).toContain("failed");
    expect(screen.getByText("Background process #4 ended")).toBeTruthy();
    expect(screen.getByText("cargo build")).toBeTruthy();
    expect(screen.getByText("src-tauri")).toBeTruthy();
    expect(screen.getByText("exit 101")).toBeTruthy();
    // News from the app, not something the agent said.
    expect(container.querySelector(".role")).toBeNull();
    fireEvent.click(card);
    expect(opened).toEqual([4]);
  });

  test("a clean exit is not drawn as a failure", () => {
    panel(state([ended(0)]));
    expect(screen.getByText("exit 0").closest(".process-ended")?.className).not.toContain("failed");
  });
});

describe("the transcript in Chat mode", () => {
  /// The same turns, calls and cards as the agent's — under the role's name.
  test("answers under the role's name, and its card answers as the agent's does", () => {
    const decided: unknown[] = [];
    render(
      <Transcript
        turn={state(
          [
            { kind: "user", id: "u0", text: "scale it" },
            { kind: "message", id: "m1", round: 1, text: "Scaling." },
            {
              kind: "approval",
              id: "approval:1",
              round: 1,
              calls: [{ id: "c1", name: "deleteFile", arguments: '{"path":"a"}', requiresConfirmation: true }],
            },
          ],
          { status: "awaitingApproval" },
        )}
        onDecide={(decisions) => decided.push(decisions)}
        speaker="Kubernetes"
      />,
    );
    expect(screen.getByText("Kubernetes")).toBeTruthy();
    expect(screen.queryByText("Agent")).toBeNull();
    fireEvent.click(screen.getByText("Allow"));
    expect(decided).toEqual([[{ id: "c1", approved: true, reason: null }]]);
  });
});
