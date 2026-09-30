import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { RuleListItem, SkillListItem, SkillsView } from "../lib/chat";

// The skills folder and a repository's AGENTS.md are edited outside the app,
// so each tab re-reads them; a switch is shown at once and then settled by
// what the backend saved.

let disk: SkillsView = { dir: "/home/.kibo/skills", skills: [] };
let calls: string[] = [];
let failToggle = false;
// Holds a switch's save until the test lets it go, to see what is shown meanwhile.
let holdToggle: Promise<void> | null = null;
let ruleFiles: RuleListItem[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: { name: string; enabled: boolean }) => {
    calls.push(command);
    if (command === "skills_list") return Promise.resolve(structuredClone(disk));
    if (command === "rules_list") return Promise.resolve(structuredClone(ruleFiles));
    if (command === "rules_set_enabled") {
      const { path, enabled } = args as unknown as { path: string; enabled: boolean };
      ruleFiles = ruleFiles.map((r) => (r.path === path ? { ...r, enabled } : r));
      return Promise.resolve();
    }
    if (command === "skills_set_source_enabled") {
      const { id, enabled } = args as unknown as { id: string; enabled: boolean };
      disk.sources = disk.sources.map((s) => (s.id === id ? { ...s, enabled } : s));
      return Promise.resolve();
    }
    if (command === "skills_set_enabled") {
      if (failToggle) return Promise.reject("settings.json is not valid");
      if (holdToggle) return holdToggle;
      disk.skills = disk.skills.map((s) => (s.name === args!.name ? { ...s, enabled: args!.enabled } : s));
      return Promise.resolve();
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useSkills } = await import("../hooks/useSkills");
const { useRules } = await import("../hooks/useRules");
const { RulesList, SkillsList, folderOf } = await import("../components/panes");
const { AsidePanel } = await import("../components/AsidePanel");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

const skill = (name: string, enabled = true, extra: Partial<SkillListItem> = {}): SkillListItem => ({
  name,
  description: `${name} does a thing`,
  enabled,
  error: null,
  source: "user",
  path: `/home/.kibo/skills/${name}`,
  shadowedBy: null,
  ...extra,
});
const theirs = (name: string, extra: Partial<SkillListItem> = {}) =>
  skill(name, true, { source: "project", path: `/repo/.claude/skills/${name}`, ...extra });

beforeEach(() => {
  disk = { dir: "/home/.kibo/skills", skills: [skill("release"), skill("review")], sources: [] };
  calls = [];
  failToggle = false;
  holdToggle = null;
  ruleFiles = [rule("AGENTS.md")];
});

function rule(name: string, extra: Partial<RuleListItem> = {}): RuleListItem {
  return { name, path: `/repo/${name}`, enabled: true, content: "Run the tests.\nUse bun.", truncated: false, error: null, ...extra };
}

describe("useSkills", () => {
  test("reads again when another folder is opened", async () => {
    const { rerender } = renderHook(({ folder }) => useSkills(true, folder), { initialProps: { folder: "/one" } });
    await settle();
    rerender({ folder: "/two" });
    await settle();
    expect(calls).toEqual(["skills_list", "skills_list"]);
  });

  test("reads the folder each time the tab opens, and not while it is hidden", async () => {
    const { result, rerender } = renderHook(({ visible }) => useSkills(visible), {
      initialProps: { visible: false },
    });
    await settle();
    expect(calls).toEqual([]);

    rerender({ visible: true });
    await settle();
    expect(result.current.view?.skills.map((s) => s.name)).toEqual(["release", "review"]);

    disk.skills.push(skill("added"));
    rerender({ visible: false });
    rerender({ visible: true });
    await settle();
    expect(result.current.view?.skills).toHaveLength(3);
  });

  test("a switch is saved and the list re-read", async () => {
    const { result } = renderHook(() => useSkills(true));
    await settle();

    await act(() => result.current.setEnabled("release", false));

    expect(calls).toEqual(["skills_list", "skills_set_enabled", "skills_list"]);
    expect(result.current.view?.skills[0].enabled).toBe(false);
  });

  test("a switch flips every row of its name at once: it is by name", async () => {
    disk = { dir: "/d", skills: [theirs("release"), skill("release"), skill("review")], sources: [] };
    let release = () => {};
    holdToggle = new Promise((resolve) => (release = resolve));
    const { result } = renderHook(() => useSkills(true));
    await settle();

    let saving: Promise<void> = Promise.resolve();
    act(() => {
      saving = result.current.setEnabled("release", false);
    });
    expect(result.current.view?.skills.map((s) => s.enabled)).toEqual([false, false, true]);
    release();
    await act(() => saving);
  });

  test("a folder switched off is saved and the list read again", async () => {
    disk.sources = [{ id: "agents", path: "/home/.agents/skills", enabled: true }];
    const { result } = renderHook(() => useSkills(true));
    await settle();

    await act(() => result.current.setSourceEnabled("agents", false));

    expect(calls).toEqual(["skills_list", "skills_set_source_enabled", "skills_list"]);
    expect(result.current.view?.sources[0].enabled).toBe(false);
  });

  test("a switch that fails says why and shows what is really saved", async () => {
    failToggle = true;
    const { result } = renderHook(() => useSkills(true));
    await settle();

    await act(() => result.current.setEnabled("release", false));

    expect(result.current.error).toContain("settings.json is not valid");
    expect(result.current.view?.skills[0].enabled).toBe(true);
  });
});

describe("the side panel", () => {
  const ctx = (active: boolean) => ({
    active,
    workspace: "/repo",
    onNotify: () => {},
    commitDraft: { message: "", onMessage: () => {} },
    processFocus: null,
    chatBlocks: [],
    mcp: {
      view: null,
      error: null,
      onToggle: () => {},
      onOpen: () => {},
      onAdd: () => {},
      onEditServer: () => {},
      onRemoveServer: () => {},
      onEditFile: () => {},
    },
    hooks: { view: null, error: null, onAdd: () => {}, onEditHook: () => {}, onRemoveHook: () => {}, onEditFile: () => {} },
    plan: { plan: null, checklist: [], onEdit: () => {}, locked: false },
  });

  test("a pane reads its own data, and only while the panel is on screen", async () => {
    const { rerender } = render(<AsidePanel tab="skills" dock="right" ctx={ctx(false)} onClose={() => {}} />);
    await settle();
    expect(calls).toEqual([]);

    rerender(<AsidePanel tab="skills" dock="right" ctx={ctx(true)} onClose={() => {}} />);
    await settle();
    expect(calls).toEqual(["skills_list"]);
    expect(screen.getByText("release")).toBeTruthy();
  });

  test("either dock closes from its own heading", () => {
    let closed = 0;
    const { rerender } = render(<AsidePanel tab="files" dock="right" ctx={ctx(true)} onClose={() => closed++} />);
    fireEvent.click(screen.getByTitle("Close panel"));
    rerender(<AsidePanel tab="files" dock="bottom" ctx={ctx(true)} onClose={() => closed++} />);
    fireEvent.click(screen.getByTitle("Close panel"));
    expect(closed).toBe(2);
  });
});

describe("the skills tab", () => {
  const panel = (view: SkillsView | null, onSkillToggle = (_: string, __: boolean) => {}) =>
    render(
      <SkillsList view={view} error={null} onToggle={onSkillToggle} />,
    );

  test("lists the skills with their switches and counts the ones on", () => {
    const toggled: [string, boolean][] = [];
    panel({ dir: "/d", skills: [skill("release"), skill("review", false)] }, (name, on) => toggled.push([name, on]));

    expect(screen.getByText("1/2")).toBeTruthy();
    const switches = screen.getAllByRole("button", { pressed: true });
    expect(switches).toHaveLength(1);
    fireEvent.click(switches[0]);
    expect(toggled).toEqual([["release", false]]);
  });

  test("the ones switched off come after the ones on, and are drawn grey", () => {
    const { container } = panel({ dir: "/d", skills: [skill("alpha", false), skill("beta"), skill("gamma", false), skill("delta")] });
    const rows = [...container.querySelectorAll(".item")];
    expect(rows.map((r) => r.querySelector(".item-title")?.textContent)).toEqual(["beta", "delta", "alpha", "gamma"]);
    expect(rows.map((r) => r.classList.contains("dim"))).toEqual([false, false, true, true]);
  });

  test("a broken skill shows its reason and has no switch", () => {
    panel({
      dir: "/d",
      skills: [skill("broken", false, { description: "", error: "SKILL.md is missing YAML frontmatter" })],
    });

    expect(screen.getByText("invalid")).toBeTruthy();
    expect(screen.getByText("SKILL.md is missing YAML frontmatter")).toBeTruthy();
    expect(screen.queryAllByRole("button", { pressed: false })).toHaveLength(0);
  });

  test("an empty folder says where skills go, the repository's folders too", () => {
    panel({ dir: "/home/.kibo/skills", skills: [] });
    expect(
      screen.getByText(/in \/home\/\.kibo\/skills, ~\/\.agents\/skills or ~\/\.claude\/skills for every repository, or in the repository's \.claude\/skills or \.agents\/skills/),
    ).toBeTruthy();
    expect(screen.queryByText("This repository")).toBeNull();
  });

  test("a copy hidden by another of the user's names that folder, not its whole path", () => {
    panel({
      dir: "/home/.kibo/skills",
      skills: [
        skill("rspack-debugging", true, { path: "/Users/me/.agents/skills/rspack-debugging" }),
        skill("rspack-debugging", true, {
          path: "/Users/me/.claude/skills/rspack-debugging",
          shadowedBy: "/Users/me/.agents/skills/rspack-debugging",
        }),
      ],
    });
    expect(screen.getByText("Used instead: ~/.agents/skills")).toBeTruthy();
  });

  test("a skill's folder is named the same on every platform", () => {
    expect(folderOf("/Users/me/.agents/skills/review")).toBe("~/.agents/skills");
    expect(folderOf("C:\\Users\\me\\.claude\\skills\\review")).toBe("~/.claude/skills");
    expect(folderOf("/opt/skills/review")).toBe("/opt/skills");
  });

  test("the repository's skills come first, and one it hides says so", () => {
    const toggled: [string, boolean][] = [];
    panel(
      {
        dir: "/home/.kibo/skills",
        skills: [
          theirs("release"),
          skill("release", true, { shadowedBy: "/repo/.claude/skills/release" }),
          skill("review", true, { path: "/Users/me/.agents/skills/review" }),
        ],
      },
      (name, on) => toggled.push([name, on]),
    );

    const [repo, mine] = screen.getAllByText(/^(This repository|Yours)$/);
    expect([repo.textContent, mine.textContent]).toEqual(["This repository", "Yours"]);
    expect(screen.getByText("1/1")).toBeTruthy();
    expect(screen.getByText("2/2")).toBeTruthy();
    expect(screen.getByText("hidden")).toBeTruthy();
    expect(screen.getByText("Used instead: this repository")).toBeTruthy();
    // Each of the user's says which folder it is from.
    expect(screen.getByText("~/.kibo/skills")).toBeTruthy();
    expect(screen.getByText("~/.agents/skills")).toBeTruthy();

    // The switch is by name, whichever row it is on.
    fireEvent.click(screen.getAllByRole("button", { pressed: true })[2]);
    expect(toggled).toEqual([["review", false]]);
  });
});

describe("useRules", () => {
  test("reads the files when the tab opens and again when the folder changes", async () => {
    const { result, rerender } = renderHook(({ visible, folder }) => useRules(visible, folder), {
      initialProps: { visible: false, folder: "/repo" as string | null },
    });
    await settle();
    expect(calls).toEqual([]);

    rerender({ visible: true, folder: "/repo" });
    await settle();
    expect(result.current.rules.map((r) => r.name)).toEqual(["AGENTS.md"]);

    ruleFiles = [rule("CLAUDE.md")];
    rerender({ visible: true, folder: "/other" });
    await settle();
    expect(result.current.rules.map((r) => r.name)).toEqual(["CLAUDE.md"]);
  });

  test("a switch is saved by path and the list re-read", async () => {
    const { result } = renderHook(() => useRules(true, "/repo"));
    await settle();

    await act(() => result.current.setEnabled("/repo/AGENTS.md", false));

    expect(calls).toEqual(["rules_list", "rules_set_enabled", "rules_list"]);
    expect(result.current.rules[0].enabled).toBe(false);
  });
});

describe("the rules tab", () => {
  const panel = (rules: RuleListItem[], onRuleToggle = (_: string, __: boolean) => {}) =>
    render(
      <RulesList rules={rules} error={null} onToggle={onRuleToggle} />,
    );

  test("a file shows its size, its text on expand, and a switch keyed by path", () => {
    const toggled: [string, boolean][] = [];
    panel([rule("AGENTS.md"), rule("CLAUDE.md", { enabled: false })], (path, on) => toggled.push([path, on]));

    expect(screen.getByText("1/2")).toBeTruthy();
    fireEvent.click(screen.getByText("AGENTS.md"));
    expect(screen.getByText(/Run the tests\./)).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { pressed: true })[0]);
    expect(toggled).toEqual([["/repo/AGENTS.md", false]]);
  });

  test("a cut file says so", () => {
    panel([rule("AGENTS.md", { truncated: true })]);
    expect(screen.getByText("cut")).toBeTruthy();
    expect(screen.getByText("2 lines sent, the rest cut")).toBeTruthy();
  });

  test("a file that cannot be sent says why and has no switch", () => {
    panel([rule("AGENTS.md", { enabled: false, content: "", error: "links outside the open folder" })]);
    expect(screen.getByText("not sent")).toBeTruthy();
    expect(screen.getByText("links outside the open folder")).toBeTruthy();
    expect(screen.queryAllByRole("button", { pressed: false })).toHaveLength(0);
  });

  test("a folder without them says which files count", () => {
    panel([]);
    expect(screen.getByText(/No AGENTS\.md or CLAUDE\.md/)).toBeTruthy();
  });
});
