import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { changesFrom } from "../lib/rewind";
import { RewindDialog } from "../components/RewindDialog";
import { ChatPanel } from "../components/ChatPanel";
import { emptyTurn, type Block } from "../lib/chatTurnReducer";
import type { FileRewind } from "../lib/chat";

const change = (path: string) => ({ path, before: { kind: "absent" as const }, after: { kind: "stored" as const, hash: path } });
const tool = (id: string, name: string, over: Partial<Extract<Block, { kind: "tool" }>> = {}): Block => ({
  kind: "tool",
  id,
  round: 1,
  name,
  arguments: "{}",
  status: "done",
  output: "",
  ...over,
});

describe("what a rewind has to undo", () => {
  const blocks: Block[] = [
    { kind: "user", id: "u0", text: "first" },
    tool("t0", "writeFile", { changes: [change("before.rs")] }),
    { kind: "user", id: "u1", text: "second" },
    tool("t1", "writeFile", { changes: [change("a.rs")] }),
    tool("t2", "runCommand"),
    tool("t3", "mcp__github__create_file"),
    tool("t4", "runCommand", { status: "failed" }),
    tool("t5", "readFile"),
    tool("t6", "editFile", { changes: [change("a.rs"), change("b.rs")] }),
  ];

  test("the changes from the message on, oldest first, and the calls nobody recorded", () => {
    expect(changesFrom(blocks, "u1")).toEqual({ changes: [change("a.rs"), change("a.rs"), change("b.rs")], unrecorded: 2 });
    expect(changesFrom(blocks, "u0").changes[0]).toEqual(change("before.rs"));
    expect(changesFrom(blocks, "nope")).toEqual({ changes: [], unrecorded: 0 });
  });
});

describe("the rewind dialog", () => {
  const file = (path: string, skip: FileRewind["skip"]): FileRewind => ({
    path,
    action: "modified",
    expected: { kind: "absent" },
    target: { kind: "absent" },
    skip,
  });

  test("lists each file with how it will go, the count, and the commands it cannot undo", () => {
    render(
      <RewindDialog
        asked={{ bubbleId: "u1", files: [file("a.rs", null), file("b.rs", "changedSince")], unrecorded: 2 }}
        onConfirm={() => {}}
        onClose={() => {}}
      />,
    );
    expect(screen.getByText("1 ready · 1 left as they are")).toBeTruthy();
    const rows = [...document.querySelectorAll(".rewind-files li")].map((li) => li.textContent);
    expect(rows).toEqual(["a.rsmodifiedready", "b.rsmodifiedchanged since — left as it is"]);
    expect(screen.getByText(/2 commands or external tools ran after it/)).toBeTruthy();
  });

  test("with no files, says so, and still asks: the messages go", () => {
    let confirmed = false;
    render(<RewindDialog asked={{ bubbleId: "u1", files: [], unrecorded: 0 }} onConfirm={() => (confirmed = true)} onClose={() => {}} />);
    expect(screen.getByText("The agent changed no files after it.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Rewind" }));
    expect(confirmed).toBe(true);
  });

  test("each way of rewinding says whether to keep a summary", () => {
    const asked: boolean[] = [];
    render(<RewindDialog asked={{ bubbleId: "u1", files: [], unrecorded: 0 }} onConfirm={(s) => asked.push(s)} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Rewind with a summary" }));
    fireEvent.click(screen.getByRole("button", { name: "Rewind" }));
    expect(asked).toEqual([true, false]);
  });

  test("while the summary is written, nothing else can be pressed", () => {
    render(<RewindDialog asked={{ bubbleId: "u1", files: [], unrecorded: 0 }} summarizing onConfirm={() => {}} onClose={() => {}} />);
    const buttons = ["Cancel", "Summarizing…", "Rewind"].map((name) => screen.getByRole("button", { name }) as HTMLButtonElement);
    expect(buttons.map((b) => b.disabled)).toEqual([true, true, true]);
  });
});

describe("the rewind button", () => {
  test("sits under a message a branch could start at, and asks for that message", () => {
    const asked: string[] = [];
    render(
      <ChatPanel
        workspace="/tmp/p"
        turn={{ ...emptyTurn(), status: "done", blocks: [{ kind: "user", id: "u0", text: "hi" }] }}
        onDecide={() => {}}
        onOpenRepo={() => {}}
        branchable={new Set(["u0"])}
        onBranch={() => {}}
        onRewind={(id) => asked.push(id)}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Rewind" }));
    expect(asked).toEqual(["u0"]);
  });
});

describe("asking for a rewind", () => {
  test("with a summary the question stays up, and cannot be closed, until the rewind is done", async () => {
    const { renderHook, act } = await import("@testing-library/react");
    const { useRewind } = await import("../hooks/useRewind");
    let finish = () => {};
    const rewound: [string, boolean | undefined][] = [];
    const { result } = renderHook(() =>
      useRewind({
        preview: async () => ({ files: [], unrecorded: 0 }),
        rewind: (id, summarize) => {
          rewound.push([id, summarize]);
          return new Promise((done) => (finish = () => done([])));
        },
        notify: () => {},
      }),
    );
    await act(() => result.current.ask("u1"));
    let confirmed: Promise<void> = Promise.resolve();
    act(() => {
      confirmed = result.current.confirm(true);
    });
    expect(result.current.summarizing).toBe(true);
    act(() => result.current.close());
    expect(result.current.asked).not.toBeNull();
    await act(async () => {
      finish();
      await confirmed;
    });
    expect(rewound).toEqual([["u1", true]]);
    expect(result.current.summarizing).toBe(false);
    expect(result.current.asked).toBeNull();
  });

  test("without one it closes at once", async () => {
    const { renderHook, act } = await import("@testing-library/react");
    const { useRewind } = await import("../hooks/useRewind");
    const rewound: [string, boolean | undefined][] = [];
    const { result } = renderHook(() =>
      useRewind({
        preview: async () => ({ files: [], unrecorded: 0 }),
        rewind: async (id, summarize) => (rewound.push([id, summarize]), []),
        notify: () => {},
      }),
    );
    await act(() => result.current.ask("u1"));
    await act(() => result.current.confirm(false));
    expect(rewound).toEqual([["u1", undefined]]);
    expect(result.current.asked).toBeNull();
  });
});
