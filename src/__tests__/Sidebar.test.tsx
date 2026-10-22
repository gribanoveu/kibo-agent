import { describe, expect, mock, test } from "bun:test";

const opened: string[] = [];
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl: async (url: string) => void opened.push(url) }));
mock.module("@tauri-apps/api/app", () => ({ getVersion: async () => "9.9.9-1" }));
import { render, screen, fireEvent } from "@testing-library/react";
import { Sidebar } from "../components/Sidebar";
import { PAGES, pagesShown } from "../components/pages";
import type { ChatSummary } from "../lib/savedChats";
import { IS_MAC } from "../lib/shortcuts";

const chats: ChatSummary[] = [
  { id: "a", title: "kept in view", updatedAt: 2, archived: false },
  { id: "b", title: "filed away", updatedAt: 1, archived: true },
];

const sidebar = (
  on: { archive?: (id: string, archived: boolean) => void; remove?: (id: string) => void; toggleTheme?: () => void } = {},
  themeSide: "light" | "dark" = "dark",
) =>
  render(
    <Sidebar
      mode="agent"
      onMode={() => {}}
      pages={PAGES}
      page={null}
      onPage={() => {}}
      chats={chats}
      activeChat={null}
      onSelectChat={() => {}}
      onNewChat={() => {}}
      onArchiveChat={on.archive ?? (() => {})}
      onDeleteChat={on.remove ?? (() => {})}
      onOpenSettings={() => {}}
      onOnboardingAction={() => {}}
      themeSide={themeSide}
      onToggleTheme={on.toggleTheme ?? (() => {})}
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
  test("switches to the other theme, named by where it goes", () => {
    let toggled = 0;
    sidebar({ toggleTheme: () => toggled++ }, "dark");
    fireEvent.click(screen.getByRole("button", { name: "Account" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Light theme" }));
    expect(toggled).toBe(1);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  test("on the light side it offers the dark one", () => {
    sidebar({}, "light");
    fireEvent.click(screen.getByRole("button", { name: "Account" }));
    expect(screen.getByRole("menuitem", { name: "Dark theme" })).toBeTruthy();
  });

  test("opens the keyboard shortcuts, and Escape closes them", () => {
    sidebar();
    fireEvent.click(screen.getByRole("button", { name: "Account" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Keyboard shortcuts" }));
    expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
    expect(screen.getByText("Show or hide the sidebar")).toBeTruthy();

    fireEvent.keyDown(document, { code: "Escape", key: "Escape" });
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

describe("pages", () => {
  const views = (page: "catalog" | null, on: { mode?: (m: string) => void; page?: (p: string) => void } = {}, board = false) =>
    render(
      <Sidebar
        mode="agent"
        onMode={on.mode ?? (() => {})}
        pages={pagesShown((feature) => board && feature === "board")}
        page={page}
        onPage={on.page ?? (() => {})}
        chats={chats}
        activeChat={null}
        onSelectChat={() => {}}
        onNewChat={() => {}}
        onArchiveChat={() => {}}
        onDeleteChat={() => {}}
        onOpenSettings={() => {}}
        onOnboardingAction={() => {}}
        themeSide="dark"
        onToggleTheme={() => {}}
      />,
    );
  const selected = () => screen.getAllByRole("tab").filter((t) => t.getAttribute("aria-selected") === "true").map((t) => t.textContent);

  test("sit under the modes, and the open one is lit instead of the mode", () => {
    const { unmount } = views(null);
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Agent", "Chat", "Catalog", "Artifacts"]);
    expect(selected()).toEqual(["Agent"]);
    unmount();
    views("catalog");
    expect(selected()).toEqual(["Catalog"]);
  });

  test("a feature's page is listed only while the feature is on", () => {
    views(null, {}, true);
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Agent", "Chat", "Catalog", "Board", "Artifacts"]);
  });

  test("open by id, and a mode is still picked from one", () => {
    const calls: string[] = [];
    views("catalog", { mode: (m) => calls.push(m), page: (p) => calls.push(p) });
    fireEvent.click(screen.getByRole("tab", { name: "Catalog" }));
    fireEvent.click(screen.getByRole("tab", { name: "Agent" }));
    expect(calls).toEqual(["catalog", "agent"]);
  });

  test("an open page drops the chat list", () => {
    const { unmount } = views(null);
    expect(screen.queryByText("Chats")).not.toBeNull();
    unmount();
    views("catalog");
    expect(screen.queryByText("Chats")).toBeNull();
  });
});
