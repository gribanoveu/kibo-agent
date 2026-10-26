import { describe, expect, test } from "bun:test";
import { ideDocks, isSavedDocks, openPane, togglePane, toggleTerminal, type Docks } from "../lib/docks";

const docks = (top: Docks["top"], topHidden: boolean, bottom: Docks["bottom"] = null): Docks => ({ top, topHidden, bottom });

describe("the Changes shortcut", () => {
  test("opens Changes when nothing is there", () => {
    expect(togglePane(docks("plan", true), "changes", "right")).toEqual(docks("changes", false));
  });

  test("closes what was there: Changes is a panel of its own", () => {
    expect(togglePane(docks("plan", false), "changes", "right")).toEqual(docks("changes", false));
    expect(togglePane(docks("overview", false, "terminal"), "changes", "right")).toEqual(docks("changes", false));
  });

  test("hides Changes, and leaves the rest", () => {
    expect(togglePane(docks("changes", false), "changes", "right")).toEqual(docks("changes", true));
    expect(togglePane(docks("changes", false, "terminal"), "changes", "right")).toEqual(docks("changes", true, "terminal"));
  });
});

describe("a pane from the menu", () => {
  test("opens when the column is free", () => {
    expect(openPane(docks("changes", true), "mcp", "right")).toEqual(docks("mcp", false));
  });

  test("replaces the pane there, and closes Terminal with Overview", () => {
    expect(openPane(docks("hooks", false), "plan", "right")).toEqual(docks("plan", false));
    expect(openPane(docks("plan", true, "terminal"), "mcp", "right")).toEqual(docks("mcp", false));
    expect(openPane(docks("overview", false, "terminal"), "mcp", "right")).toEqual(docks("mcp", false));
  });

  test("already on screen stays where it is, so it is never drawn twice", () => {
    const state = docks("overview", false, "terminal");
    expect(openPane(state, "terminal", "bottom")).toBe(state);
    expect(openPane(state, "overview", "right")).toBe(state);
  });
});

describe("Overview and Terminal", () => {
  test("open one under the other, as one panel", () => {
    expect(openPane(docks("overview", false), "terminal", "bottom")).toEqual(docks("overview", false, "terminal"));
    expect(openPane(docks("overview", true, "terminal"), "overview", "right")).toEqual(docks("overview", false, "terminal"));
    expect(openPane(docks("plan", true), "terminal", "bottom")).toEqual(docks("plan", true, "terminal"));
  });

  test("replace a panel of its own instead of going beside it", () => {
    expect(openPane(docks("mcp", false), "overview", "right")).toEqual(docks("overview", false));
    expect(openPane(docks("mcp", false), "terminal", "bottom")).toEqual(docks("mcp", true, "terminal"));
  });

  test("a pane left under another by an older version goes when Overview opens", () => {
    expect(openPane(docks("plan", true, "changes"), "overview", "right")).toEqual(docks("overview", false));
  });
});

describe("the Terminal button", () => {
  test("opens Terminal under Overview, and closes only it", () => {
    expect(toggleTerminal(docks("overview", false))).toEqual(docks("overview", false, "terminal"));
    expect(toggleTerminal(docks("overview", false, "terminal"))).toEqual(docks("overview", false, null));
  });

  test("in place of a panel of its own", () => {
    expect(toggleTerminal(docks("plan", false))).toEqual(docks("plan", true, "terminal"));
  });
});

describe("the IDE layout", () => {
  const swapping = (top: Docks["top"], topHidden: boolean, bottom: Docks["bottom"] = null): Docks => ({
    ...docks(top, topHidden, bottom),
    swap: true,
  });

  test("a pane takes the top's place instead of going under it; Terminal still goes under", () => {
    expect(openPane(swapping("files", false, "terminal"), "plan", "right")).toMatchObject(docks("plan", false, "terminal"));
    expect(togglePane(swapping("files", false), "changes", "right")).toMatchObject(docks("changes", false));
    expect(togglePane(swapping("plan", false), "files", "right")).toMatchObject(docks("files", false));
    expect(toggleTerminal(swapping("files", false))).toMatchObject(docks("files", false, "terminal"));
  });

  test("opens on the tree, keeping only Terminal under the file", () => {
    expect(ideDocks(docks("changes", true, "plan"))).toEqual(docks("files", false));
    expect(ideDocks(docks("mcp", false, "terminal"))).toEqual(docks("files", false, "terminal"));
  });
});

describe("the panes kept from before the IDE layout", () => {
  test("come back whole or not at all", () => {
    expect(isSavedDocks(null)).toBe(true);
    expect(isSavedDocks(docks("plan", false, "terminal"))).toBe(true);
    expect(isSavedDocks(docks("changes", true))).toBe(true);
    for (const broken of ["plan", { top: "gone", topHidden: false, bottom: null }, { top: "plan", bottom: null }, { top: "plan", topHidden: true, bottom: "gone" }])
      expect(isSavedDocks(broken)).toBe(false);
  });
});
