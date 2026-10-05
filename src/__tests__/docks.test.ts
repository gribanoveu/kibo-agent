import { describe, expect, test } from "bun:test";
import { changesShown, docsDocks, openPane, togglePane, toggleChanges, toggleTerminal, type Docks } from "../lib/docks";

const docks = (top: Docks["top"], topHidden: boolean, bottom: Docks["bottom"] = null): Docks => ({ top, topHidden, bottom });

describe("the Changes button", () => {
  test("opens Changes on top when nothing is there", () => {
    expect(toggleChanges(docks("plan", true))).toEqual(docks("changes", false));
  });

  test("does not replace another pane on top: Changes goes under it", () => {
    const next = toggleChanges(docks("plan", false, "terminal"));
    expect(next).toEqual(docks("plan", false, "changes"));
    expect(changesShown(next)).toBe(true);
  });

  test("hides Changes wherever it is, and leaves the rest", () => {
    expect(toggleChanges(docks("changes", false, "terminal"))).toEqual(docks("changes", true, "terminal"));
    expect(toggleChanges(docks("plan", false, "changes"))).toEqual(docks("plan", false, null));
  });
});

describe("a pane from the menu", () => {
  test("opens on top when the top is free", () => {
    expect(openPane(docks("changes", true), "mcp", "right")).toEqual(docks("mcp", false));
    expect(openPane(docks("changes", true, "terminal"), "mcp", "right")).toEqual(docks("mcp", false, "terminal"));
  });

  test("does not replace the pane on top: it goes under it, in place of what was there", () => {
    expect(openPane(docks("hooks", false), "plan", "right")).toEqual(docks("hooks", false, "plan"));
    expect(openPane(docks("hooks", false, "plan"), "mcp", "right")).toEqual(docks("hooks", false, "mcp"));
  });

  test("a bottom pane goes under even when the top is free", () => {
    expect(openPane(docks("plan", true), "terminal", "bottom")).toEqual(docks("plan", true, "terminal"));
  });

  test("already on screen stays where it is, so it is never drawn twice", () => {
    const state = docks("plan", false, "changes");
    expect(openPane(state, "changes", "right")).toBe(state);
    expect(openPane(state, "plan", "right")).toBe(state);
  });
});

describe("the Terminal button", () => {
  test("opens Terminal under whatever is on top, and closes only it", () => {
    expect(toggleTerminal(docks("plan", false, "changes"))).toEqual(docks("plan", false, "terminal"));
    expect(toggleTerminal(docks("plan", true))).toEqual(docks("plan", true, "terminal"));
    expect(toggleTerminal(docks("plan", false, "terminal"))).toEqual(docks("plan", false, null));
  });
});

describe("the docs layout", () => {
  const swapping = (top: Docks["top"], topHidden: boolean, bottom: Docks["bottom"] = null): Docks => ({
    ...docks(top, topHidden, bottom),
    swap: true,
  });

  test("a pane takes the top's place instead of going under it; Terminal still goes under", () => {
    expect(openPane(swapping("files", false, "terminal"), "plan", "right")).toMatchObject(docks("plan", false, "terminal"));
    expect(toggleChanges(swapping("files", false))).toMatchObject(docks("changes", false));
    expect(togglePane(swapping("plan", false), "files", "right")).toMatchObject(docks("files", false));
    expect(toggleTerminal(swapping("files", false))).toMatchObject(docks("files", false, "terminal"));
  });

  test("opens on the tree, keeping only Terminal under the file", () => {
    expect(docsDocks(docks("changes", true, "plan"))).toEqual(docks("files", false));
    expect(docsDocks(docks("mcp", false, "terminal"))).toEqual(docks("files", false, "terminal"));
  });
});
