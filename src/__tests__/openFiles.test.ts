import { describe, expect, test } from "bun:test";
import { fileLinkPath, openFilesReducer, stepThrough, viewerReducer, type OpenFiles } from "../hooks/useOpenFiles";
import type { FileTarget } from "../lib/chat";

// The viewer's tabs: one per file and side, a single preview tab that the
// next single click reuses, and where the view goes when a tab is closed.

const a: FileTarget = { path: "a.rs", side: "unstaged" };
const aStaged: FileTarget = { path: "a.rs", side: "staged" };
const b: FileTarget = { path: "b.rs", side: "worktree" };
const c: FileTarget = { path: "c.rs", side: "worktree" };

type Step = Parameters<typeof openFilesReducer>[1];
const empty: OpenFiles = { files: [], active: null, preview: null };
const run = (actions: Step[], from: OpenFiles = empty) => actions.reduce(openFilesReducer, from);
const click = (target: FileTarget): Step => ({ kind: "open", target, pin: false });
const keep = (target: FileTarget): Step => ({ kind: "open", target, pin: true });

describe("open files", () => {
  test("single clicks down a list reuse one preview tab, in its place", () => {
    const state = run([keep(a), click(b), click(c)]);
    expect(state).toEqual({ files: [a, c], active: c, preview: c });
  });

  test("a double click keeps the tab: the next single click opens beside it", () => {
    // A double click is a click, then a pin of the same file.
    const state = run([click(b), keep(b), click(c)]);
    expect(state).toEqual({ files: [b, c], active: c, preview: c });
    expect(run([click(b), { kind: "pin", target: b }, click(c)]).files).toEqual([b, c]);
  });

  test("opening a file already open only shows it, and leaves the preview where it is", () => {
    const state = run([keep(a), click(b), click({ ...a })]);
    expect(state).toEqual({ files: [a, b], active: a, preview: b });
  });

  test("the same path from another side is another tab", () => {
    expect(run([keep(a), keep(aStaged)]).files).toEqual([a, aStaged]);
  });

  test("closing the tab showing moves to the next, or the one before when it was last", () => {
    const three = run([keep(a), keep(b), keep(c)]);
    expect(run([{ kind: "close", target: b }], { ...three, active: b })).toEqual({ files: [a, c], active: c, preview: null });
    expect(run([{ kind: "close", target: c }], three)).toEqual({ files: [a, b], active: b, preview: null });
    expect(run([a, b, c].map((target) => ({ kind: "close", target }) as Step), three)).toEqual(empty);
  });

  test("closing the preview tab leaves none; the next single click opens a new one", () => {
    const state = run([keep(a), click(b), { kind: "close", target: b }, click(c)]);
    expect(state).toEqual({ files: [a, c], active: c, preview: c });
  });

  test("closing another tab keeps the one showing; closing one not open changes nothing", () => {
    const three = run([keep(a), keep(b), keep(c)]);
    expect(run([{ kind: "close", target: a }], three)).toEqual({ files: [b, c], active: c, preview: null });
    expect(run([{ kind: "close", target: aStaged }], three)).toBe(three);
    expect(run([{ kind: "closeAll" }], three)).toEqual(empty);
  });
});

describe("each layout's own tabs", () => {
  type ViewerStep = Parameters<typeof viewerReducer>[1];
  const viewer = (actions: ViewerStep[]) => actions.reduce(viewerReducer, { shown: empty, other: empty });

  test("switching puts the tabs away and brings back the other layout's, as they were left", () => {
    const chat = viewer([keep(a), click(b)]);
    const ide = viewer([keep(a), click(b), { kind: "swap" }]);
    expect(ide.shown).toEqual(empty);
    const back = [keep(c), { kind: "swap" } as const].reduce(viewerReducer, ide);
    expect(back.shown).toEqual(chat.shown);
    expect([{ kind: "swap" } as const].reduce(viewerReducer, back).shown).toEqual({ files: [c], active: c, preview: null });
  });

  test("Close all closes the tabs on screen; another folder empties both", () => {
    const put = viewer([keep(a), { kind: "swap" }, keep(c)]);
    expect(viewerReducer(put, { kind: "closeAll" })).toEqual({ shown: empty, other: { files: [a], active: a, preview: null } });
    expect(viewerReducer(put, { kind: "reset" })).toEqual({ shown: empty, other: empty });
  });
});

describe("stepping through changed files", () => {
  const list = [a, aStaged, b];

  test("forward and back, going round at the ends", () => {
    expect(stepThrough(list, a, 1)).toEqual(aStaged);
    expect(stepThrough(list, b, 1)).toEqual(a);
    expect(stepThrough(list, a, -1)).toEqual(b);
    expect(stepThrough(list, aStaged, -1)).toEqual(a);
  });

  test("from a file not in the list, forward is the first and back the last", () => {
    expect(stepThrough(list, c, 1)).toEqual(a);
    expect(stepThrough(list, c, -1)).toEqual(b);
    expect(stepThrough([], a, 1)).toBeNull();
  });
});

describe("fileLinkPath", () => {
  const root = "/Users/me/repo";
  const cases: Array<[link: string, path: string | null]> = [
    ["src/a.ts", "src/a.ts"],
    ["./src/a.ts", "src/a.ts"],
    ["src/a.ts:42", "src/a.ts"],
    ["README.md:42:7", "README.md"],
    ["src/a.ts#L42", "src/a.ts"],
    ["/Users/me/repo/src/a.ts", "src/a.ts"],
    ["file:///Users/me/repo/src/a.ts", "src/a.ts"],
    ["my%20notes.md", "my notes.md"],
    ["/etc/passwd", null],
    ["../other/a.ts", null],
    ["src/../../a.ts", null],
    ["./", null],
  ];
  for (const [link, path] of cases) test(`${link} → ${path}`, () => expect(fileLinkPath(link, root)).toBe(path));
});
