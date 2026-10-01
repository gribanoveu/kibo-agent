import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { Block } from "../lib/chatTurnReducer";
import type { TurnEvent } from "../lib/chat";

// The plan is a document the user reads and edits between turns. What must
// hold: the agent's newest plan reaches the tab, the user's edit reaches the
// next turn and the saved chat, and an old plan in the transcript never
// overwrites an edit made after it.

type Call = { command: string; args: Record<string, unknown> };
const calls: Call[] = [];
let emit: (event: TurnEvent) => void = () => {};
/** What the next `chat_start` "does" before it returns: the events it emits. */
let during: TurnEvent[] = [];
let record: unknown = null;
/** When set, `chat_start` stays running until it resolves. */
let hold: Promise<void> | null = null;
/** When set, `chat_start` ends as a turn the user stopped. */
let stopped = false;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "chat_start") {
      for (const event of during) emit({ ...event, turnId: args.turnId as string });
      return (hold ?? Promise.resolve()).then(() => ({ status: stopped ? "cancelled" : "done", value: { text: "", truncated: false, todos: [], history: args.messages } }));
    }
    if (command === "chat_load") return Promise.resolve(record);
    return Promise.resolve(null);
  },
}));
mock.module("@tauri-apps/api/event", () => ({
  listen: (_: string, handler: (message: { payload: TurnEvent }) => void) => {
    emit = (event) => handler({ payload: event });
    return Promise.resolve(() => {});
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useAgentTurn } = await import("../hooks/useAgentTurn");
const { writtenChecklist, writtenPlan } = await import("../lib/plan");
const { PlanPanel, currentTask } = await import("../components/PlanPanel");
const { describeTool } = await import("../lib/describeTool");

afterEach(() => {
  calls.length = 0;
  during = [];
  record = null;
  hold = null;
  stopped = false;
});

const tool = (id: string, name: string, args: unknown, status: "done" | "failed" = "done"): Block => ({
  kind: "tool",
  id,
  round: 1,
  name,
  arguments: JSON.stringify(args),
  status,
  output: "",
});

/** The events of a turn in which the model wrote `content` as the plan. */
const writes = (content: string, id = "p1"): TurnEvent[] => [
  { turnId: "", seq: 1, round: 1, type: "toolCall", payload: { id, name: "writePlan", arguments: JSON.stringify({ content }) } },
  { turnId: "", seq: 2, round: 1, type: "toolResult", payload: { id, result: { result: "planWritten", lines: 1 } } },
];

const lastSave = () => calls.filter((c) => c.command === "chat_save").at(-1)?.args;
const lastStart = () => calls.filter((c) => c.command === "chat_start").at(-1)?.args;

describe("writtenPlan", () => {
  test("the newest plan that was written, not one that failed", () => {
    expect(
      writtenPlan([
        tool("a", "writePlan", { content: "# One" }),
        tool("b", "todo", { op: "write" }),
        tool("c", "writePlan", { content: "  # Two\n" }),
        tool("d", "writePlan", { content: "# Refused" }, "failed"),
      ]),
    ).toBe("# Two");
  });

  test("none written is none", () => {
    expect(writtenPlan([tool("a", "readFile", { path: "a.rs" })])).toBeNull();
    expect(writtenPlan([{ ...tool("a", "writePlan", {}), arguments: "{broken" } as Block])).toBeNull();
  });
});

describe("writtenChecklist", () => {
  const todo = (id: string, tasks: unknown, status: "done" | "failed" = "done"): Block => ({
    ...(tool(id, "todo", { op: "write" }, status) as Extract<Block, { kind: "tool" }>),
    result: { result: "todo", tasks },
  });

  test("the list the last todo call that ran returned", () => {
    const one = [{ id: "t1", title: "Read", status: "inProgress" }];
    const two = [{ id: "t1", title: "Read", status: "completed" }];
    expect(writtenChecklist([todo("a", one), todo("b", two), todo("c", [], "failed")])).toEqual(two as never);
  });

  test("no todo call is none", () => {
    expect(writtenChecklist([tool("a", "readFile", { path: "a.rs" })])).toBeNull();
  });
});

describe("the plan through a conversation", () => {
  test("the checklist reaches the tab while the turn still runs", async () => {
    let release = () => {};
    hold = new Promise((resolve) => (release = resolve));
    const tasks = [{ id: "t1", title: "Read the parser", status: "inProgress" }];
    during = [
      { turnId: "", seq: 1, round: 1, type: "toolCall", payload: { id: "d1", name: "todo", arguments: '{"op":"write"}' } },
      { turnId: "", seq: 2, round: 1, type: "toolResult", payload: { id: "d1", result: { result: "todo", tasks } } },
    ];
    const { result } = renderHook(() => useAgentTurn());
    let sent: Promise<void> = Promise.resolve();
    await act(async () => {
      sent = result.current.send("do it");
    });

    expect(result.current.turn.status).toBe("running");
    expect(result.current.checklist).toEqual(tasks as never);
    await act(async () => {
      release();
      await sent;
    });
  });

  test("a plan the agent wrote reaches the tab, the saved chat and the next turn", async () => {
    const { result } = renderHook(() => useAgentTurn());
    during = writes("# Fix the parser");
    await act(async () => result.current.send("plan it"));

    expect(result.current.plan).toBe("# Fix the parser");
    expect(lastSave()?.plan).toBe("# Fix the parser");

    await act(async () => result.current.send("go on"));
    expect(lastStart()?.plan).toBe("# Fix the parser");
  });

  test("the user's edit is what the next turn gets — and a turn that writes none keeps it", async () => {
    const { result } = renderHook(() => useAgentTurn());
    during = writes("# Draft");
    await act(async () => result.current.send("plan it"));

    act(() => result.current.editPlan("# Draft, corrected"));
    expect(lastSave()?.plan).toBe("# Draft, corrected");

    during = [];
    await act(async () => result.current.send("looks right?"));
    expect(lastStart()?.plan).toBe("# Draft, corrected");
    // The old writePlan is still in the transcript; it must not win.
    expect(result.current.plan).toBe("# Draft, corrected");
    expect(lastSave()?.plan).toBe("# Draft, corrected");
  });

  test("a reopened chat brings its plan and its checklist; a new chat has neither", async () => {
    record = {
      id: "c1",
      messages: [],
      blocks: [tool("old", "writePlan", { content: "# Old" })],
      todos: [{ id: "t1", title: "read", status: "pending" }],
      plan: "# Edited later",
    };
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => result.current.open("c1"));
    expect(result.current.plan).toBe("# Edited later");
    expect(result.current.checklist).toHaveLength(1);

    await act(async () => result.current.send("continue"));
    expect(lastSave()?.plan).toBe("# Edited later");

    act(() => result.current.reset());
    expect(result.current.plan).toBeNull();
    expect(result.current.checklist).toEqual([]);
  });

  /// The window opens the Plan tab on each count — once per plan, as it is
  /// written, never for a chat that is only being opened.
  test("each plan written is counted; opening and plain turns are not", async () => {
    record = { id: "c1", messages: [], blocks: [tool("old", "writePlan", { content: "# Old" })], todos: [], plan: "# Old" };
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => result.current.open("c1"));
    expect(result.current.planWritten).toBe(0);

    during = writes("# One");
    await act(async () => result.current.send("plan it"));
    expect(result.current.planWritten).toBe(1);

    during = [];
    await act(async () => result.current.send("thanks"));
    expect(result.current.planWritten).toBe(1);

    // Shown as it was written, so stopping the turn afterwards takes nothing back.
    during = writes("# Half", "p-half");
    stopped = true;
    await act(async () => result.current.send("stop midway"));
    expect(result.current.turn.status).toBe("cancelled");
    expect(result.current.planWritten).toBe(2);
    stopped = false;

    // A call id is unique across the conversation; a repeated one would be the first call again.
    during = writes("# Two", "p2");
    await act(async () => result.current.send("again"));
    expect(result.current.planWritten).toBe(3);
  });

  /// A long turn can spend most of its time carrying the plan out; the user
  /// reads it while it does, not after.
  test("a plan is on the tab while its turn is still running", async () => {
    let release: () => void = () => {};
    hold = new Promise((resolve) => (release = resolve));
    during = writes("# Now");
    const { result } = renderHook(() => useAgentTurn());

    let sent: Promise<void> = Promise.resolve();
    await act(async () => {
      sent = result.current.send("plan it");
    });
    expect(result.current.turn.status).toBe("running");
    expect(result.current.plan).toBe("# Now");
    expect(result.current.planWritten).toBe(1);

    await act(async () => {
      release();
      await sent;
    });
    expect(result.current.planWritten).toBe(1);
  });

  test("a chat saved before plans existed opens without one", async () => {
    record = { id: "c1", messages: [], blocks: [], todos: [] };
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => result.current.open("c1"));
    expect(result.current.plan).toBeNull();
  });
});

describe("the Plan tab", () => {
  const tab = (props: Partial<Parameters<typeof PlanPanel>[0]> = {}) =>
    render(<PlanPanel plan="# Fix" checklist={[]} onEdit={() => {}} locked={false} {...props} />);

  test("the plan is read as Markdown until the user asks to edit it", () => {
    tab({ plan: "# Fix the parser\n\n- read the grammar" });
    expect(screen.getByRole("heading", { name: "Fix the parser" })).toBeTruthy();
    expect(screen.queryByLabelText("Plan")).toBeNull();
    fireEvent.click(screen.getByLabelText("Edit plan"));
    expect((screen.getByLabelText("Plan") as HTMLTextAreaElement).value).toBe("# Fix the parser\n\n- read the grammar");
  });

  test("an edit is handed over when the user leaves the text, not per keystroke", () => {
    const edits: string[] = [];
    const { rerender } = tab({ onEdit: (p) => edits.push(p) });
    fireEvent.click(screen.getByLabelText("Edit plan"));
    const text = screen.getByLabelText("Plan");

    fireEvent.change(text, { target: { value: "# Fix\n\n1. first" } });
    expect(edits).toEqual([]);
    fireEvent.blur(text);
    expect(edits).toEqual(["# Fix\n\n1. first"]);

    // The parent keeps the edit; leaving the text again changes nothing.
    rerender(<PlanPanel plan="# Fix\n\n1. first" checklist={[]} onEdit={(p) => edits.push(p)} locked={false} />);
    fireEvent.blur(text);
    expect(edits).toHaveLength(1);
  });

  test("Done and Escape hand the edit over and go back to reading", () => {
    const edits: string[] = [];
    tab({ onEdit: (p) => edits.push(p) });
    fireEvent.click(screen.getByLabelText("Edit plan"));
    fireEvent.change(screen.getByLabelText("Plan"), { target: { value: "# One" } });
    fireEvent.click(screen.getByText("Done"));
    expect(edits).toEqual(["# One"]);
    expect(screen.queryByLabelText("Plan")).toBeNull();

    fireEvent.click(screen.getByLabelText("Edit plan"));
    fireEvent.change(screen.getByLabelText("Plan"), { target: { value: "# Two" } });
    fireEvent.keyDown(screen.getByLabelText("Plan"), { key: "Escape" });
    expect(edits).toEqual(["# One", "# Two"]);
    expect(screen.queryByLabelText("Plan")).toBeNull();
  });

  test("with no plan yet, one can be written from the empty panel", () => {
    tab({ plan: null });
    expect(screen.queryByText("Checklist")).toBeNull();
    fireEvent.click(screen.getByText("write your own"));
    expect(screen.getByLabelText("Plan")).toBeTruthy();
  });

  test("while a turn runs the plan cannot be edited", () => {
    tab({ locked: true });
    expect((screen.getByLabelText("Edit plan") as HTMLButtonElement).disabled).toBe(true);
  });

  test("the handover is offered only with a plan to hand over", () => {
    let handed = 0;
    const { unmount } = tab({ plan: null, onImplement: () => handed++ });
    expect(screen.queryByText("Implement in Agent mode")).toBeNull();
    unmount();
    tab({ onImplement: () => handed++ });
    fireEvent.click(screen.getByText("Implement in Agent mode"));
    expect(handed).toBe(1);
  });

  test("the checklist shows where the work stands", () => {
    tab({
      checklist: [
        { id: "1", title: "read", status: "completed" },
        { id: "2", title: "fix", status: "inProgress" },
        { id: "3", title: "rename", status: "cancelled", note: "not needed" },
      ],
    });
    expect(screen.getByText("1/3")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Checklist" }));
    expect(screen.getByText("not needed")).toBeTruthy();
    expect(screen.getByText("fix").closest("li")?.className).toContain("inProgress");
    expect(screen.getByLabelText("Done")).toBeTruthy();
  });
});

describe("the folded checklist", () => {
  const task = (id: string, status: "completed" | "inProgress" | "pending" | "cancelled") => ({ id, title: `task ${id}`, status });
  const panel = (checklist: ReturnType<typeof task>[]) =>
    render(<PlanPanel plan="# Fix" checklist={checklist} onEdit={() => {}} locked={false} />);

  test("shows only the task under way until opened, then all of them", () => {
    panel([task("1", "completed"), task("2", "inProgress"), task("3", "pending")]);
    expect(screen.getAllByRole("listitem").map((li) => li.textContent)).toEqual(["task 2"]);
    expect(screen.getByRole("button", { name: "Checklist" }).getAttribute("aria-expanded")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "Checklist" }));
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
    fireEvent.click(screen.getByRole("button", { name: "Checklist" }));
    expect(screen.getAllByRole("listitem")).toHaveLength(1);
  });

  test("the one in progress over the next to do; the next to do when none has started", () => {
    expect(currentTask([task("1", "pending"), task("2", "inProgress")])?.id).toBe("2");
    expect(currentTask([task("1", "completed"), task("2", "cancelled"), task("3", "pending")])?.id).toBe("3");
    expect(currentTask([task("1", "completed"), task("2", "cancelled")])).toBeNull();
  });

  test("nothing left is said as such", () => {
    panel([task("1", "completed"), task("2", "cancelled")]);
    expect(screen.getByRole("listitem").textContent).toBe("All done");
  });

  /// The entrance plays because the row is a new element, not the old one relabelled.
  test("a new current task is a new row", () => {
    const { rerender } = panel([task("1", "inProgress"), task("2", "pending")]);
    const first = screen.getByRole("listitem");
    rerender(<PlanPanel plan="# Fix" checklist={[task("1", "completed"), task("2", "inProgress")]} onEdit={() => {}} locked={false} />);
    const second = screen.getByRole("listitem");
    expect(second.textContent).toBe("task 2");
    expect(second).not.toBe(first);
    expect(second.className).toContain("plan-current");
  });
});

test("a writePlan call in the transcript is shown by its title", () => {
  const block = tool("p1", "writePlan", { content: "# Fix the parser\n\n1. read" }) as Extract<Block, { kind: "tool" }>;
  const shown = describeTool({ ...block, result: { result: "planWritten", lines: 3 } });
  expect(shown).toMatchObject({ name: "Plan", arg: "Fix the parser", meta: "3 lines" });
});
