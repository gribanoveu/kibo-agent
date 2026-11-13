import { describe, expect, mock, test } from "bun:test";

const opened: string[] = [];
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl: async (url: string) => void opened.push(url) }));
mock.module("@tauri-apps/api/app", () => ({ getVersion: async () => "9.9.9-1" }));
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { Sidebar } from "../components/Sidebar";
import { PAGES, pagesShown } from "../components/pages";
import type { ChatGroup, ChatSummary } from "../lib/savedChats";
import { IS_MAC } from "../lib/shortcuts";
import type { ChatActivity } from "../types";

const chats: ChatSummary[] = [
  { id: "a", title: "kept in view", updatedAt: 2, archived: false },
  { id: "b", title: "filed away", updatedAt: 1, archived: true },
];

type Handlers = {
  archive?: (id: string, archived: boolean) => void;
  pin?: (id: string, pinned: boolean) => void;
  move?: (id: string, groupId: string | null) => void;
  createGroup?: (name: string) => Promise<{ id?: string; error?: string }>;
  renameGroup?: (id: string, name: string) => Promise<{ error?: string }>;
  deleteGroup?: (id: string) => void;
  remove?: (id: string) => void;
  select?: (id: string) => void;
  toggleTheme?: () => void;
};

const sidebar = (
  on: Handlers = {},
  themeSide: "light" | "dark" = "dark",
  chatActivity: Record<string, ChatActivity> = {},
  rows: ChatSummary[] = chats,
  groups: ChatGroup[] = [],
) =>
  render(
    <Sidebar
      mode="agent"
      onMode={() => {}}
      pages={PAGES}
      page={null}
      onPage={() => {}}
      chats={rows}
      groups={groups}
      groupScope="/repo"
      activeChat={null}
      chatActivity={chatActivity}
      onSelectChat={on.select ?? (() => {})}
      onArchiveChat={on.archive ?? (() => {})}
      onPinChat={on.pin ?? (() => {})}
      onMoveChat={on.move ?? (() => {})}
      onCreateGroup={on.createGroup ?? (async () => ({}))}
      onRenameGroup={on.renameGroup ?? (async () => ({}))}
      onDeleteGroup={on.deleteGroup ?? (() => {})}
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

  /// What was touched today comes under its own heading; a heading with nothing under it is not drawn.
  test("sorts the list by day", () => {
    const now = Date.now();
    sidebar({}, "dark", {}, [
      { id: "t", title: "from today", updatedAt: now, archived: false },
      { id: "e", title: "from before", updatedAt: now - 3 * 86_400_000, archived: false },
    ]);
    const under = (day: string) => screen.getByText(day).closest("section")?.textContent;
    expect(under("Today")).toContain("from today");
    expect(under("Today")).not.toContain("from before");
    expect(under("Earlier")).toContain("from before");
  });

  /// Typed, the list narrows to what matches; Escape gives it all back.
  test("the search narrows the list by title, and Escape clears it", () => {
    sidebar();
    const box = screen.getByRole("textbox", { name: "Search chats" });
    fireEvent.change(box, { target: { value: "KEPT" } });
    expect(screen.getByText("kept in view")).toBeTruthy();

    fireEvent.change(box, { target: { value: "nothing like it" } });
    expect(screen.queryByText("kept in view")).toBeNull();
    expect(screen.getByText("No chats match.")).toBeTruthy();

    fireEvent.keyDown(box, { code: "Escape", key: "Escape" });
    expect(screen.getByText("kept in view")).toBeTruthy();
  });

  test("a day with no chats has no heading", () => {
    sidebar();
    expect(screen.queryByText("Today")).toBeNull();
    expect(screen.getByText("Earlier")).toBeTruthy();
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

  test("distinguishes a running turn, an approval wait and a background finish", () => {
    const selected: string[] = [];
    const rows = [...chats, { id: "c", title: "done away", updatedAt: 3, archived: false }];
    sidebar({ select: (id) => selected.push(id) }, "dark", { a: "running", b: "awaitingApproval", c: "finished" }, rows);
    pickFilter("All");

    const running = screen.getByLabelText("Turn running");
    expect(running).toBeTruthy();
    expect(screen.getByLabelText("Approval needed")).toBeTruthy();
    expect(screen.getByLabelText("Finished in background")).toBeTruthy();
    const row = running.closest("button");
    expect(row?.firstElementChild).toBe(running);
    expect(row?.querySelectorAll("svg")).toHaveLength(1);
    fireEvent.click(screen.getByText("done away"));
    expect(selected).toEqual(["c"]);
  });

  describe("pinned chats", () => {
    const rows = (): ChatSummary[] => [
      { id: "a", title: "kept in view", updatedAt: 2, archived: false },
      { id: "p", title: "kept on top", updatedAt: 1, archived: false, pinned: true },
      { id: "b", title: "filed away", updatedAt: 3, archived: true },
    ];

    /// The pinned group holds the active filter's pinned rows ahead of the
    /// days, and the days never repeat one: a row shown twice reads as two chats.
    test("stands under Pinned, not under its day", () => {
      sidebar({}, "dark", {}, rows());
      const under = (heading: string) => screen.queryByText(heading)?.closest("section")?.textContent ?? "";
      expect(under("Pinned")).toContain("kept on top");
      expect(under("Today")).not.toContain("kept on top");
      expect(under("Earlier")).not.toContain("kept on top");
      expect(screen.getByLabelText("Pinned")).toBeTruthy();
    });

    test("a pinned row offers to sink back, an unpinned one to hold", () => {
      const calls: [string, boolean][] = [];
      sidebar({ pin: (id, pinned) => calls.push([id, pinned]) }, "dark", {}, rows());
      pickFilter("All");

      const [first, second] = screen.getAllByTitle("More");
      fireEvent.click(first);
      fireEvent.click(screen.getByRole("menuitem", { name: "Pin" }));
      fireEvent.click(second);
      fireEvent.click(screen.getByRole("menuitem", { name: "Unpin" }));

      expect(calls).toEqual([
        ["a", true],
        ["p", false],
      ]);
    });

    /// The pinned group is the working view; "All" is for finding, and
    /// there a pinned chat stays a row of its day.
    test("has no group in All: rows stay under their days", () => {
      sidebar({}, "dark", {}, rows());
      pickFilter("All");
      expect(screen.queryByText("Pinned")).toBeNull();
      const under = (day: string) => screen.getByText(day).closest("section")?.textContent ?? "";
      expect(under("Earlier")).toContain("kept on top");
    });

    test("the search narrows the pinned group like the days", () => {
      sidebar({}, "dark", {}, rows());
      const box = screen.getByRole("textbox", { name: "Search chats" });
      fireEvent.change(box, { target: { value: "on top" } });
      expect(screen.getByText("kept on top")).toBeTruthy();
      expect(screen.queryByText("kept in view")).toBeNull();

      fireEvent.change(box, { target: { value: "nothing like it" } });
      expect(screen.queryByText("Pinned")).toBeNull();
    });
  });

  describe("chat folders", () => {
    const groupList: ChatGroup[] = [
      { id: "g1", name: "release", position: 1 },
      { id: "g2", name: "launch", position: 2 },
    ];
    const rows = (): ChatSummary[] => [
      { id: "a", title: "in release", updatedAt: 3, archived: false, group: "g1" },
      { id: "b", title: "loose", updatedAt: 2, archived: false },
      { id: "c", title: "filed away", updatedAt: 1, archived: true },
    ];

    /// A chat lives either in its folder or in the days — a row shown twice
    /// reads as two chats, the rule the pinned group set.
    test("a foldered chat stands under its folder, not in the days", () => {
      sidebar({}, "dark", {}, rows(), groupList);
      const under = (heading: string) => screen.queryByText(heading)?.closest("section")?.textContent ?? "";
      expect(under("release")).toContain("in release");
      expect(under("Today")).not.toContain("in release");
      expect(under("Earlier")).not.toContain("in release");
    });

    /// A folder made ahead of its first chat stays reachable: listed, with
    /// an Empty line where its rows will go.
    test("an empty folder is drawn with an Empty line", () => {
      sidebar({}, "dark", {}, rows(), groupList);
      const launch = screen.getByText("launch").closest("section");
      expect(launch?.textContent).toContain("Empty");
    });

    /// Alive folders below make a "No chats yet." above them a contradiction
    /// — their Empty lines are the state.
    test("no chats and live folders: no contradictory empty line", () => {
      sidebar({}, "dark", {}, [], groupList);
      expect(screen.queryByText("No chats yet.")).toBeNull();
      expect(screen.getByText("release")).toBeTruthy();
    });

    /// Folders are the working view: All and Archived stay plain lists.
    test("has no folders outside Active: a foldered chat is a row of its day", () => {
      sidebar({}, "dark", {}, rows(), groupList);
      pickFilter("All");
      expect(screen.queryByText("launch")).toBeNull();
      const under = (day: string) => screen.getByText(day).closest("section")?.textContent ?? "";
      expect(under("Earlier")).toContain("in release");
    });

    test("a collapsed folder keeps its heading and count but hides the rows", () => {
      localStorage.setItem("kibo-chat-groups-/repo", JSON.stringify(["g1"]));
      try {
        sidebar({}, "dark", {}, rows(), groupList);
        const release = screen.getByText("release").closest("section");
        expect(release?.textContent).toContain("release");
        expect(release?.textContent).not.toContain("in release");

        fireEvent.click(screen.getByRole("button", { name: /release/ }));
        expect(screen.getByText("in release")).toBeTruthy();
      } finally {
        localStorage.removeItem("kibo-chat-groups-/repo");
      }
    });

    test("moving from the row: No folder unmoves, a folder moves by id", () => {
      const moves: [string, string | null][] = [];
      sidebar({ move: (id, group) => moves.push([id, group]) }, "dark", {}, rows(), groupList);

      const rowOf = (title: string) => screen.getByText(title).closest(".chat-row") as HTMLElement;
      fireEvent.click(within(rowOf("in release")).getByTitle("More"));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "To folder" }));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "No folder" }));
      expect(moves).toEqual([["a", null]]);

      fireEvent.click(within(rowOf("in release")).getByTitle("More"));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "To folder" }));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "launch" }));
      expect(moves).toEqual([
        ["a", null],
        ["a", "g2"],
      ]);
    });

    /// A folder made from the move menu is made for the chat being moved:
    /// created, then the chat goes into it.
    test("a new folder made from the move menu takes the chat", async () => {
      const moves: [string, string | null][] = [];
      const created: string[] = [];
      sidebar(
        {
          move: (id, group) => moves.push([id, group]),
          createGroup: async (name) => {
            created.push(name);
            return { id: "g9" };
          },
        },
        "dark",
        {},
        rows(),
        groupList,
      );

      const rowOf = (title: string) => screen.getByText(title).closest(".chat-row") as HTMLElement;
      fireEvent.click(within(rowOf("in release")).getByTitle("More"));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "To folder" }));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "New folder…" }));
      fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "hotfix" } });
      fireEvent.click(screen.getByRole("button", { name: "Create" }));

      await waitFor(() => expect(moves).toEqual([["a", "g9"]]));
      expect(created).toEqual(["hotfix"]);
      expect(screen.queryByRole("dialog")).toBeNull();
    });

    /// The backend refuses a name another folder carries; the refusal is
    /// shown in the dialog, which stays open for another try.
    test("a refused name keeps the dialog open with the error", async () => {
      sidebar({ createGroup: async () => ({ error: 'a folder named "release" already exists' }) }, "dark", {}, rows(), groupList);
      const rowOf = (title: string) => screen.getByText(title).closest(".chat-row") as HTMLElement;
      fireEvent.click(within(rowOf("in release")).getByTitle("More"));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "To folder" }));
      fireEvent.click(within(rowOf("in release")).getByRole("menuitem", { name: "New folder…" }));
      fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "release" } });
      fireEvent.click(screen.getByRole("button", { name: "Create" }));

      expect(await screen.findByText('a folder named "release" already exists')).toBeTruthy();
      expect(screen.getByRole("dialog", { name: "New folder" })).toBeTruthy();
    });

    /// A chat pointing at a folder the window does not know — deleted
    /// mid-move, folders list staler than the chats — is never lost: it
    /// falls into the days rather than vanish from the working view.
    test("a chat whose folder is unknown falls into the days", () => {
      sidebar({}, "dark", {}, [{ id: "x", title: "orphan", updatedAt: 2, archived: false, group: "gone" }], groupList);
      const under = (heading: string) => screen.queryByText(heading)?.closest("section")?.textContent ?? "";
      expect(under("Earlier")).toContain("orphan");
      expect(under("release")).not.toContain("orphan");
    });

    /// Searching hides a folder with nothing matching, as it does a day —
    /// an "Empty" that means "didn't match" reads as a lost folder.
    test("searching hides a folder with no matches instead of calling it Empty", () => {
      sidebar({}, "dark", {}, rows(), groupList);
      fireEvent.change(screen.getByRole("textbox", { name: "Search chats" }), { target: { value: "loose" } });
      expect(screen.queryByText("launch")).toBeNull();
      expect(screen.getByText("loose")).toBeTruthy();
    });

    /// Pinning an archived chat changes a state no list shows until it is
    /// back from the archive — the row's menu keeps to what works there.
    test("an archived row offers no Pin", () => {
      sidebar({}, "dark", {}, rows(), groupList);
      pickFilter("Archived");
      const row = screen.getByText("filed away").closest(".chat-row") as HTMLElement;
      fireEvent.click(within(row).getByTitle("More"));
      expect(within(row).queryByRole("menuitem", { name: "Pin" })).toBeNull();
      expect(within(row).getByRole("menuitem", { name: "Unarchive" })).toBeTruthy();
    });

    test("a folder is renamed and deleted from its own menu", async () => {      const renamed: [string, string][] = [];
      const deleted: string[] = [];
      sidebar(
        {
          renameGroup: async (id, name) => {
            renamed.push([id, name]);
            return {};
          },
          deleteGroup: (id) => deleted.push(id),
        },
        "dark",
        {},
        rows(),
        groupList,
      );

      const headOf = (name: string) => screen.getByText(name).closest(".chat-folder-head") as HTMLElement;
      fireEvent.click(within(headOf("release")).getByTitle("More"));
      fireEvent.click(within(headOf("release")).getByRole("menuitem", { name: "Rename…" }));
      fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "launch-13" } });
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(renamed).toEqual([["g1", "launch-13"]]));

      fireEvent.click(within(headOf("launch")).getByTitle("More"));
      fireEvent.click(within(headOf("launch")).getByRole("menuitem", { name: "Delete…" }));
      fireEvent.click(screen.getByRole("button", { name: "Delete" }));
      expect(deleted).toEqual(["g2"]);
    });
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
        groups={[]}
        groupScope=""
        activeChat={null}
        onSelectChat={() => {}}
        onArchiveChat={() => {}}
        onPinChat={() => {}}
        onMoveChat={() => {}}
        onCreateGroup={async () => ({})}
        onRenameGroup={async () => ({})}
        onDeleteGroup={() => {}}
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
    expect(selected()).toEqual(["Agent", "Catalog"]);
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

  /// A page changes what is right of the sidebar, not the sidebar.
  test("an open page keeps the search and the list, with no chat marked open", () => {
    views("catalog");
    expect(screen.getByRole("textbox", { name: "Search chats" })).toBeTruthy();
    expect(screen.getByText("Chats")).toBeTruthy();
    expect(document.querySelector(".chat.active")).toBeNull();
  });
});
