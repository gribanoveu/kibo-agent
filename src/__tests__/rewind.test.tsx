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
