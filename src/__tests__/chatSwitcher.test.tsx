import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { ChatSwitcher } from "../components/ChatSwitcher";
import type { ChatSummary } from "../lib/chat";

const chat = (id: string, title: string, archived = false): ChatSummary => ({ id, title, updatedAt: 0, archived });
const chats = [chat("a", "Document the API"), chat("b", "Old one", true), chat("c", "Fix the index")];

// The IDE layout's menu of chats, in the chat header in place of the sidebar.
describe("ChatSwitcher", () => {
  test("lists a new chat first, then the folder's chats that are not archived; the open one is checked", () => {
    render(<ChatSwitcher title="Fix the index" chats={chats} activeChat="c" onSelect={() => {}} onNew={() => {}} />);
    fireEvent.click(screen.getByTitle("Switch chats"));
    const options = screen.getAllByRole("option");
    expect(options.map((o) => o.querySelector(".dropdown-item-label")?.textContent)).toEqual([
      "New chat",
      "Document the API",
      "Fix the index",
    ]);
    expect(options[2].getAttribute("aria-selected")).toBe("true");
  });

  test("a chat opens by its id, the first row starts a new one", () => {
    const picked: string[] = [];
    render(<ChatSwitcher title={null} chats={chats} activeChat={null} onSelect={(id) => picked.push(id)} onNew={() => picked.push("new")} />);
    const trigger = screen.getByTitle("Switch chats");
    expect(trigger.textContent).toBe("New chat");
    fireEvent.click(trigger);
    // Not saved yet: the new chat's row is the one checked.
    expect(screen.getAllByRole("option")[0].getAttribute("aria-selected")).toBe("true");
    fireEvent.click(screen.getByText("Document the API"));
    fireEvent.click(trigger);
    fireEvent.click(screen.getAllByRole("option")[0]);
    expect(picked).toEqual(["a", "new"]);
  });
});
