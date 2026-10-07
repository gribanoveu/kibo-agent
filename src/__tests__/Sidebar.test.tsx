import { describe, expect, mock, test } from "bun:test";

const opened: string[] = [];
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl: async (url: string) => void opened.push(url) }));
mock.module("@tauri-apps/api/app", () => ({ getVersion: async () => "9.9.9-1" }));
import { render, screen, fireEvent } from "@testing-library/react";
import { Sidebar } from "../components/Sidebar";
import type { ChatSummary } from "../lib/chat";
import { IS_MAC } from "../lib/shortcuts";

const chats: ChatSummary[] = [
  { id: "a", title: "kept in view", updatedAt: 2, archived: false },
  { id: "b", title: "filed away", updatedAt: 1, archived: true },
];

const sidebar = (on: { archive?: (id: string, archived: boolean) => void; remove?: (id: string) => void } = {}) =>
  render(
    <Sidebar
      chats={chats}
      activeChat={null}
      onSelectChat={() => {}}
      onNewChat={() => {}}
      onArchiveChat={on.archive ?? (() => {})}
      onDeleteChat={on.remove ?? (() => {})}
      onToggleCollapse={() => {}}
      onOpenSettings={() => {}}
      onOnboardingAction={() => {}}
    />,
  );

const pickFilter = (label: string) => {
  fireEvent.click(screen.getByTitle("Which chats to list"));
  fireEvent.click(screen.getByRole("option", { name: label }));
};

describe("the chat list", () => {
  test("keeps archived chats out of sight until they are asked for", () => {
    sidebar();
    expect(screen.getByText("kept in view")).toBeTruthy();
    expect(screen.queryByText("filed away")).toBeNull();

    pickFilter("Archived");
    expect(screen.queryByText("kept in view")).toBeNull();
    expect(screen.getByText("filed away")).toBeTruthy();

    pickFilter("All");
    expect(screen.getByText("kept in view")).toBeTruthy();
    expect(screen.getByText("filed away")).toBeTruthy();
  });

  test("archives from a row's menu, and an archived row offers to bring it back", () => {
    const calls: [string, boolean][] = [];
    sidebar({ archive: (id, archived) => calls.push([id, archived]) });
    pickFilter("All");

    const [first, second] = screen.getAllByTitle("More");
    fireEvent.click(first);
    fireEvent.click(screen.getByRole("menuitem", { name: "Archive" }));
    fireEvent.click(second);
    fireEvent.click(screen.getByRole("menuitem", { name: "Unarchive" }));

    expect(calls).toEqual([
      ["a", true],
      ["b", false],
    ]);
  });

  test("deletes only once it is confirmed", () => {
    const removed: string[] = [];
    sidebar({ remove: (id) => removed.push(id) });

    fireEvent.click(screen.getByTitle("More"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(removed).toEqual([]);

    fireEvent.click(screen.getByTitle("More"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(removed).toEqual(["a"]);
  });
});

describe("the account menu", () => {
  test("opens the keyboard shortcuts, and Escape closes them", () => {
    sidebar();
    fireEvent.click(screen.getByRole("button", { name: "Account" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Keyboard shortcuts" }));
    expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
    expect(screen.getByText("Show or hide the sidebar")).toBeTruthy();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("the shortcut for the shortcuts opens and closes them from anywhere", () => {
    sidebar();
    const press = () => fireEvent.keyDown(window, { key: "/", code: "Slash", ...(IS_MAC ? { metaKey: true } : { ctrlKey: true }) });
    press();
    expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
    press();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("about", () => {
  test("shows the running version and where to download a new one", async () => {
    sidebar();
    fireEvent.click(screen.getByRole("button", { name: /Account/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: /About Kibo Agent/ }));
    expect(await screen.findByText("Version 9.9.9-1")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Download page" }));
    expect(opened).toEqual(["https://github.com/gribanoveu/kibo-agent/releases"]);
  });
});
