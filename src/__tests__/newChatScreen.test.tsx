import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";

// A new chat's screen is the wordmark and nothing else: the folder, its branch
// and what the message goes out with are all in the card under it, so nothing
// here repeats them. With no folder open it has one thing to say.

mock.module("@tauri-apps/api/core", () => ({ invoke: () => Promise.resolve(null) }));

const { ChatEmptyState } = await import("../components/ChatEmptyState");

beforeEach(() => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("a new chat's screen", () => {
  test("is the wordmark alone with a folder open — no title, no second folder", () => {
    render(<ChatEmptyState workspace="/work/kibo" onOpenRepo={() => {}} />);
    expect(document.querySelector(".chat-empty .kibo-brand")).toBeTruthy();
    expect(screen.queryByText("Nothing is open")).toBeNull();
    expect(screen.queryByText("kibo")).toBeNull();
    // It already is a new chat: offering another one does nothing.
    expect(screen.queryByRole("button")).toBeNull();
  });

  test("with nothing open, it says so and offers the one way in", () => {
    let picks = 0;
    render(<ChatEmptyState workspace={null} onOpenRepo={() => picks++} />);
    expect(screen.getByText("Nothing is open")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Open folder/ }));
    expect(picks).toBe(1);
  });

  /// The first thing the screen asks for is one press, not a dialog: the
  /// folders opened lately, three at most, and the picker for the rest.
  test("with nothing open, the lately opened folders are a press each", () => {
    const opened: string[] = [];
    render(
      <ChatEmptyState
        workspace={null}
        onOpenRepo={() => {}}
        recent={[
          { path: "/work/kibo", worktreeOf: null },
          { path: "/work/atlas", worktreeOf: null },
          { path: "/work/kibo-feature", worktreeOf: "/work/kibo" },
          { path: "/work/old", worktreeOf: null },
        ]}
        onOpenFolder={(path) => opened.push(path)}
      />,
    );
    const rows = [...document.querySelectorAll(".chat-empty-recent-folder")];
    expect(rows.map((one) => one.textContent)).toEqual(["kibo", "atlas", "kibo-feature"]);
    expect(rows[2].getAttribute("title")).toContain("A worktree of /work/kibo");
    fireEvent.click(rows[1]);
    expect(opened).toEqual(["/work/atlas"]);
    // The picker is still there, for a folder that is not in the list.
    expect(screen.getByRole("button", { name: /Open folder/ })).toBeTruthy();
  });

  test("with nothing opened lately, only the picker is offered", () => {
    render(<ChatEmptyState workspace={null} onOpenRepo={() => {}} recent={[]} onOpenFolder={() => {}} />);
    expect(document.querySelector(".chat-empty-recent")).toBeNull();
    expect(screen.getByRole("button", { name: /Open folder/ })).toBeTruthy();
  });
});
