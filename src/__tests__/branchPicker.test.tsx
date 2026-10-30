import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { useState } from "react";
import type { CheckoutOutcome } from "../lib/git";

// A chat not yet started picks its branch. The app switches only when git
// can do it without overwriting anything; otherwise it says which files are
// in the way and offers a worktree — it never stashes, forces or merges.

let outcome: CheckoutOutcome = { kind: "switched" };
let failure: string | null = null;
const calls: { command: string; args: unknown }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    calls.push({ command, args });
    if (failure) return Promise.reject(failure);
    if (command === "git_branches")
      return Promise.resolve([
        { name: "main", remote: false },
        { name: "origin/feature", remote: true },
      ]);
    if (command === "git_checkout") return Promise.resolve(outcome);
    if (command === "git_worktree_add") return Promise.resolve("/home/.kibo/worktrees/kibo/main");
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});
afterEach(() => {
  outcome = { kind: "switched" };
  failure = null;
  calls.length = 0;
});

const { useBranchPicker } = await import("../hooks/useBranchPicker");
const { FolderTab } = await import("../components/FolderTab");
const { BranchConflictDialog } = await import("../components/BranchConflictDialog");
const { useFolderConversation } = await import("../hooks/useFolderConversation");

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const commands = () => calls.map((call) => call.command);

type Setup = { agree?: boolean };
const picker = ({ agree = true }: Setup = {}) => {
  const opened: string[] = [];
  const notes: string[] = [];
  let guarded = 0;
  const hook = renderHook(() =>
    useBranchPicker({
      chatId: null,
      current: "main",
      guard: (go, onCancel) => {
        guarded++;
        if (agree) go();
        else onCancel?.();
      },
      open: async (path) => {
        opened.push(path);
        return true;
      },
      notify: (message) => notes.push(message),
      send: () => {},
    }),
  );
  return { hook, opened, notes, guarded: () => guarded };
};

describe("picking a branch", () => {
  test("switches the folder when nothing is in the way, and not at all for the branch already out", async () => {
    const { hook, opened } = picker();
    await act(() => hook.result.current.pick("main"));
    expect(commands()).toEqual([]);

    await act(() => hook.result.current.pick("origin/feature"));
    expect(calls).toEqual([{ command: "git_checkout", args: { branch: "origin/feature" } }]);
    expect(hook.result.current.conflict).toBeNull();
    expect(opened).toEqual([]);
  });

  test("holds a refused switch with the files in the way, and does nothing else about them", async () => {
    outcome = { kind: "conflicts", paths: ["src/a.ts", "notes.md"] };
    const { hook } = picker();
    await act(() => hook.result.current.pick("feature"));
    expect(hook.result.current.conflict).toEqual({ branch: "feature", paths: ["src/a.ts", "notes.md"] });
    expect(commands()).toEqual(["git_checkout"]);

    act(() => hook.result.current.closeConflict());
    expect(hook.result.current.conflict).toBeNull();
  });

  test("with Worktree on, a pick is only where the worktree will start", async () => {
    const { hook, guarded } = picker();
    expect(hook.result.current.base).toBe("main");
    act(() => hook.result.current.setWorktree(true));
    await act(() => hook.result.current.pick("origin/feature"));
    expect(hook.result.current.base).toBe("origin/feature");
    expect(commands()).toEqual([]);
    expect(guarded()).toBe(0);

    // Off again, the choice is forgotten: on again starts from the folder's own branch.
    act(() => hook.result.current.setWorktree(false));
    act(() => hook.result.current.setWorktree(true));
    expect(hook.result.current.base).toBe("main");
  });

  /// Picked for a new chat, the tick is that chat's: going back to another and
  /// then to a new chat again does not start a worktree nobody asked for there.
  test("Worktree and its base are dropped when another chat is opened", async () => {
    const hook = renderHook(
      ({ chatId }) =>
        useBranchPicker({ chatId, current: "main", guard: (go) => go(), open: async () => true, notify: () => {}, send: () => {} }),
      { initialProps: { chatId: null as string | null } },
    );
    act(() => hook.result.current.setWorktree(true));
    await act(() => hook.result.current.pick("feature"));
    expect(hook.result.current.base).toBe("feature");

    hook.rerender({ chatId: "chat-a" });
    expect(hook.result.current.worktree).toBe(false);
    expect(hook.result.current.base).toBe("main");
  });

  test("starting one makes it after the folder switch is agreed to, opens it, and turns Worktree off", async () => {
    const { hook, opened, guarded } = picker();
    act(() => hook.result.current.setWorktree(true));
    await act(() => hook.result.current.pick("feature"));
    let started: boolean | null = null;
    await act(async () => {
      started = await hook.result.current.startWorktree("feature");
    });
    expect(started).toBe(true);
    expect(guarded()).toBe(1);
    expect(calls).toEqual([{ command: "git_worktree_add", args: { base: "feature" } }]);
    expect(opened).toEqual(["/home/.kibo/worktrees/kibo/main"]);
    expect(hook.result.current.worktree).toBe(false);
    expect(hook.result.current.base).toBe("main");
  });

  /// A worktree nobody opens would be left on disk; the message goes back to the box.
  test("makes none and says so when leaving the folder is not agreed to", async () => {
    const { hook, opened } = picker({ agree: false });
    let started: boolean | null = null;
    await act(async () => {
      started = await hook.result.current.startWorktree("main");
    });
    expect(started).toBe(false);
    expect(commands()).toEqual([]);
    expect(opened).toEqual([]);
  });

  /// Sent into the old folder, the agent would work where the user meant to keep out of; sent
  /// before the switch's fresh thread, it would be wiped by it.
  test("the first message is sent in the worktree, after the folder switch has started its thread", async () => {
    const log: string[] = [];
    const agent = { turn: { blocks: [] }, open: () => {}, reset: () => log.push("reset") };
    const hook = renderHook(() => {
      const [folder, setFolder] = useState("/work/kibo");
      useFolderConversation(folder, false, undefined, agent);
      const picker = useBranchPicker({
        chatId: null,
        current: "main",
        guard: (go) => go(),
        open: async (path) => {
          log.push(`open ${path}`);
          setFolder(path);
          return true;
        },
        notify: () => {},
        send: (text) => log.push(`send "${text}" in ${folder}`),
      });
      return picker;
    });
    await act(async () => {
      await hook.result.current.startWorktree("main", "fix the parser");
    });
    await settle();
    expect(log).toEqual([
      "open /home/.kibo/worktrees/kibo/main",
      "reset",
      'send "fix the parser" in /home/.kibo/worktrees/kibo/main',
    ]);
  });

  test("nothing is sent when the worktree does not open", async () => {
    const sent: string[] = [];
    const { result } = renderHook(() =>
      useBranchPicker({
        chatId: null,
        current: "main",
        guard: (_go, onCancel) => onCancel?.(),
        open: async () => true,
        notify: () => {},
        send: (text) => sent.push(text),
      }),
    );
    let started: boolean | null = null;
    await act(async () => {
      started = await result.current.startWorktree("main", "fix it");
    });
    await settle();
    expect(started).toBe(false);
    expect(sent).toEqual([]);
  });

  test("says it did not start one when git refuses", async () => {
    failure = "no branch named gone";
    const { hook, notes, opened } = picker();
    let started: boolean | null = null;
    await act(async () => {
      started = await hook.result.current.startWorktree("gone");
    });
    expect(started).toBe(false);
    expect(notes).toEqual([failure]);
    expect(opened).toEqual([]);
  });

  test("a conflict's way round is a worktree from the branch it was refused", async () => {
    outcome = { kind: "conflicts", paths: ["a.txt"] };
    const { hook, opened } = picker();
    await act(() => hook.result.current.pick("feature"));
    await act(async () => {
      await hook.result.current.startWorktree(hook.result.current.conflict!.branch);
    });
    expect(hook.result.current.conflict).toBeNull();
    expect(calls.at(-1)).toEqual({ command: "git_worktree_add", args: { base: "feature" } });
    expect(opened).toHaveLength(1);
  });

  test("says why when git refuses", async () => {
    failure = "main is checked out in /elsewhere — open that folder instead";
    const { hook, notes } = picker();
    await act(() => hook.result.current.pick("feature"));
    act(() => hook.result.current.load());
    await settle();
    expect(notes).toEqual([failure, failure]);
    expect(hook.result.current.conflict).toBeNull();
  });
});

describe("the folder tab's branch", () => {
  type Picker = NonNullable<Parameters<typeof FolderTab>[0]["branchPicker"]>;
  const tab = (branchPicker?: Picker, chatBranch?: string | null) =>
    render(
      <FolderTab
        path="/work/kibo"
        recent={[]}
        branch="main"
        onOpenFolder={() => {}}
        onPickFolder={() => {}}
        branchPicker={branchPicker}
        chatBranch={chatBranch}
      />,
    );
  const started = (over: Partial<Picker> = {}): Picker => ({
    branches: [
      { name: "main", remote: false },
      { name: "feature", remote: false },
    ],
    onOpen: () => {},
    onPick: () => {},
    worktree: false,
    ...over,
  });

  test("without a picker, is only shown", () => {
    tab();
    expect(screen.getByTitle("Checked-out branch").textContent).toBe("main");
    expect(screen.queryByRole("checkbox", { name: /Worktree/ })).toBeNull();
  });

  test("before it has, lists the branches as they are opened and says which are remote", () => {
    const picked: string[] = [];
    let opened = 0;
    let worktree: boolean | null = null;
    tab({
      branches: [
        { name: "main", remote: false },
        { name: "origin/feature", remote: true },
      ],
      onOpen: () => opened++,
      onPick: (name) => picked.push(name),
      worktree: false,
      onWorktree: (on) => (worktree = on),
    });
    fireEvent.click(screen.getByTitle("Switch the folder's branch"));
    expect(opened).toBe(1);
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["main", "origin/featureremote"]);
    fireEvent.click(screen.getByRole("option", { name: /origin\/feature/ }));
    expect(picked).toEqual(["origin/feature"]);

    const toggle = screen.getByRole("checkbox", { name: "Worktree" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    expect(worktree).toBe(true);
  });

  /// A worktree is a new folder and a new chat in it: nothing a started chat can become.
  test("once the chat has started, still switches between turns, with no Worktree", () => {
    const picked: string[] = [];
    tab(started({ onPick: (name) => picked.push(name) }));
    expect(screen.queryByRole("checkbox", { name: /Worktree/ })).toBeNull();
    fireEvent.click(screen.getByTitle("Switch the folder's branch"));
    fireEvent.click(screen.getByRole("option", { name: "feature" }));
    expect(picked).toEqual(["feature"]);
  });

  /// Switched under a running turn, the agent's next edit would land on another branch.
  test("is locked while a turn works on it, saying why", () => {
    tab(started({ locked: "The agent is working on this branch" }));
    fireEvent.click(screen.getByTitle("The agent is working on this branch"));
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  /// Another chat of the folder switched it: the label shows what is checked
  /// out, and this chat's own branch beside it, one click away.
  test("a chat the folder was moved off says which branch is its own, and switches back", () => {
    const picked: string[] = [];
    tab(started({ onPick: (name) => picked.push(name) }), "feature");
    expect(screen.getByTitle("Switch the folder's branch").textContent).toBe("main");
    fireEvent.click(screen.getByTitle(/This chat works on feature; the folder is on main now/));
    expect(picked).toEqual(["feature"]);
  });

  test("a chat on the branch checked out has nothing to say about it", () => {
    tab(started(), "main");
    expect(screen.queryByTitle(/This chat works on/)).toBeNull();
  });

  test("with Worktree on, the menu asks where the worktree starts and shows that branch", () => {
    tab({ branches: [], onOpen: () => {}, onPick: () => {}, worktree: true, base: "feature", onWorktree: () => {} });
    expect(screen.getByTitle("The branch a new worktree starts from").textContent).toContain("feature");
    expect(screen.getByRole("checkbox", { name: "Worktree" }).getAttribute("title")).toContain("from feature");
    expect(screen.getByRole("checkbox", { name: "Worktree" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByTitle("The branch a new worktree starts from"));
    expect(screen.getByText("Start a worktree from")).toBeTruthy();
  });
});

describe("a folder that is a worktree", () => {
  const tab = (worktreeOf: string | null) =>
    render(
      <FolderTab
        path="/home/.kibo/worktrees/kibo/main-2"
        recent={[]}
        branch="kibo/main-2"
        worktreeOf={worktreeOf}
        onOpenFolder={() => {}}
        onPickFolder={() => {}}
      />,
    );

  test("is named after its repository, marked, and both folders are on hover", () => {
    tab("/work/kibo");
    const folder = screen.getByTitle(/A worktree of/);
    expect(folder.textContent).toBe("kiboworktree");
    expect(folder.getAttribute("title")).toBe("A worktree of /work/kibo\nThis folder: /home/.kibo/worktrees/kibo/main-2");
  });

  test("an ordinary folder carries no mark", () => {
    tab(null);
    expect(screen.queryByText("worktree")).toBeNull();
    expect(screen.getByTitle("/home/.kibo/worktrees/kibo/main-2").textContent).toBe("main-2");
  });
});

describe("the conflict dialog", () => {
  test("lists the files and offers a worktree from the branch", () => {
    const started: string[] = [];
    render(
      <BranchConflictDialog
        conflict={{ branch: "feature", paths: ["src/a.ts", "notes.md"] }}
        onWorktree={(branch) => started.push(branch)}
        onClose={() => {}}
      />,
    );
    expect(screen.getAllByRole("listitem").map((li) => li.textContent)).toEqual(["src/a.ts", "notes.md"]);
    fireEvent.click(screen.getByRole("button", { name: "Start a worktree" }));
    expect(started).toEqual(["feature"]);
  });

  test("is closed with no conflict", () => {
    render(<BranchConflictDialog conflict={null} onWorktree={() => {}} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
