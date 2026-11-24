import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { FeatureView } from "../lib/features";

// The app's own features on the Catalog page (docs/33-features.md): a card
// beside the catalog's entries, turned on and off, its data deleted only
// while off and only after asking.

let board: FeatureView;
let calls: { command: string; args?: Record<string, unknown> }[] = [];
/** What a switch refuses with, as a rejected write does. */
let switchFails: string | null = null;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "catalog_list") return Promise.resolve([]);
    if (command === "code_hosts") return Promise.resolve([]);
    if (command === "features_list") return Promise.resolve([structuredClone(board)]);
    if (command === "features_set") {
      board = { ...board, on: args!.on as boolean };
      return Promise.resolve([structuredClone(board)]);
    }
    if (command === "features_set_tools") {
      if (switchFails) return Promise.reject(switchFails);
      board = { ...board, toolsOn: args!.on as boolean };
      return Promise.resolve([structuredClone(board)]);
    }
    if (command === "features_set_layout_switch") {
      board = { ...board, layoutSwitchOn: args!.on as boolean };
      return Promise.resolve([structuredClone(board)]);
    }
    if (command === "features_delete_data") {
      board = { ...board, hasData: false };
      return Promise.resolve([structuredClone(board)]);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Set<() => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: () => void) => {
    if (channel === "features:changed") listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { CatalogPage } = await import("../components/CatalogPage");
const { useFeatures } = await import("../hooks/useFeatures");
const { renderHook } = await import("@testing-library/react");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  calls = [];
  switchFails = null;
  board = {
    id: "board",
    name: "Board",
    description: "Cards for later",
    on: false,
    tools: [{ name: "board", description: "When the user asks you to remind them…" }],
    toolsOn: true,
    layoutSwitchOn: false,
    layoutSwitchSupported: true,
    places: ["Assistant chat", "Agent", "Analyst"],
    data: "~/.kibo/board.db",
    hasData: true,
  };
});

describe("features on the catalog page", () => {
  test("a feature is a card under Features, and its panel says what the model gets and where", async () => {
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    const section = within(screen.getByRole("region", { name: "Features" }));
    expect(section.getByText("Built into the app")).toBeTruthy();

    fireEvent.click(section.getByRole("button", { name: "Board" }));
    const panel = within(screen.getByRole("complementary", { name: "Board details" }));
    // The sheet opens on Settings: what the feature is and where its data is.
    expect(panel.getByRole("tab", { name: "Settings" }).getAttribute("aria-selected")).toBe("true");
    expect(panel.getByText("~/.kibo/board.db")).toBeTruthy();
    // One panel at a time: nothing of the other tab is here.
    expect(panel.queryByText("When the user asks you to remind them…")).toBeNull();

    // What the model gets, and where it is offered, is the Tools tab's.
    fireEvent.click(panel.getByRole("tab", { name: "Tools" }));
    expect(panel.getByRole("tab", { name: "Tools" }).getAttribute("aria-selected")).toBe("true");
    expect(panel.getByRole("tab", { name: "Settings" }).getAttribute("aria-selected")).toBe("false");
    expect(panel.getByText("Where: Assistant chat, Agent, Analyst")).toBeTruthy();
    expect(panel.getByText("When the user asks you to remind them…")).toBeTruthy();
    // And the settings are gone: the tab replaced them, rather than stacking on them.
    expect(panel.queryByText("~/.kibo/board.db")).toBeNull();
  });

  test("a feature with no tools says so on its Tools tab, and one with a page says what it adds", async () => {
    board = { ...board, id: "provider-balance", name: "Provider balance", tools: [], places: [], data: null, hasData: false };
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Provider balance" }));
    fireEvent.click(screen.getByRole("tab", { name: "Tools" }));
    expect(screen.getByText("No tools for the model: everything this feature adds is for you in the window.")).toBeTruthy();
    // The tab stays, with that line instead of an empty list.
    expect(screen.getByRole("tabpanel", { name: "Tools" })).toBeTruthy();
  });

  test("a feature whose tools bring a page says so where its tools would be", async () => {
    // The board is the one of the test's features with a page of its own (pages.tsx).
    board = { ...board, tools: [] };
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    fireEvent.click(screen.getByRole("tab", { name: "Tools" }));
    expect(screen.getByText("No tools for the model: this feature adds its page to the sidebar, and nothing to the model.")).toBeTruthy();
  });

  test("+ turns it on, and the panel turns it off again", async () => {
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByLabelText("Turn on Board"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_set", args: { id: "board", on: true } });
    expect(screen.getByLabelText("On")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    // Bitbucket's servers belong to Pull requests' panel, not to every feature that is on.
    expect(screen.queryByRole("region", { name: "Bitbucket servers" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Turn off" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_set", args: { id: "board", on: false } });
  });

  test("the panel's switch keeps the tools from the model and leaves the feature on", async () => {
    board = { ...board, on: true };
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    // The switch is about the tools, so it is set among them.
    fireEvent.click(screen.getByRole("tab", { name: "Tools" }));
    const tools = () => screen.getByRole("switch", { name: "Tools for the model" });
    expect(tools().getAttribute("aria-checked")).toBe("true");
    fireEvent.click(tools());
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_set_tools", args: { id: "board", on: false } });
    expect(tools().getAttribute("aria-checked")).toBe("false");
    fireEvent.click(screen.getByRole("tab", { name: "Settings" }));
    expect(screen.getByRole("button", { name: "Turn off" })).toBeTruthy();
  });

  test("a switch that fails says so on the tab it was thrown from", async () => {
    board = { ...board, on: true };
    switchFails = "the settings file could not be written";
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    fireEvent.click(screen.getByRole("tab", { name: "Tools" }));
    fireEvent.click(screen.getByRole("switch", { name: "Tools for the model" }));
    await settle();
    // The message is not a tab's own: it is where it was clicked.
    expect(screen.getByText("the settings file could not be written")).toBeTruthy();
  });

  test("Bitbucket takes its servers in its own panel, once it is on", async () => {
    board = { ...board, id: "pull-requests", name: "Bitbucket", tools: [], places: [], data: "code_hosts.enc" };
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(within(screen.getByRole("region", { name: "Features" })).getByRole("button", { name: "Bitbucket" }));
    await settle();
    expect(screen.queryByRole("region", { name: "Bitbucket servers" })).toBeNull();

    fireEvent.click(screen.getByText("Turn on"));
    await settle();
    const servers = within(screen.getByRole("region", { name: "Bitbucket servers" }));
    expect(servers.getByText(/None yet/)).toBeTruthy();
    fireEvent.click(servers.getByText("Add server"));
    expect(screen.getByRole("dialog", { name: "Connect Bitbucket" })).toBeTruthy();
  });

  test("the layout feature's optional system switch is saved from its panel", async () => {
    board = { ...board, id: "layout-correction", name: "Keyboard layout correction", on: true, tools: [] };
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Keyboard layout correction" }));
    const toggle = () => screen.getByRole("switch", { name: "Switch keyboard layout after Fix" });
    expect(toggle().getAttribute("aria-checked")).toBe("false");

    fireEvent.click(toggle());
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_set_layout_switch", args: { on: true } });
    expect(toggle().getAttribute("aria-checked")).toBe("true");
  });

  test("its data is deleted only while it is off, and only after asking", async () => {
    board = { ...board, on: true };
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    expect(screen.queryByRole("button", { name: "Delete data" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Turn off" }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Delete data" }));
    expect(calls.some((c) => c.command === "features_delete_data")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_delete_data", args: { id: "board" } });
    expect(screen.queryByRole("button", { name: "Delete data" })).toBeNull();
  });

  test("the Features tab shows only features, and search narrows them", async () => {
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByRole("tab", { name: "Features" }));
    expect(screen.getByRole("region", { name: "Features" })).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Search the catalog"), { target: { value: "nothing like it" } });
    expect(screen.queryByRole("region", { name: "Features" })).toBeNull();
  });
});

describe("useFeatures", () => {
  test("reads the list again when a feature is turned on elsewhere", async () => {
    const { result } = renderHook(() => useFeatures());
    await settle();
    expect(result.current.isOn("board")).toBe(false);
    board = { ...board, on: true };
    act(() => listeners.forEach((handler) => handler()));
    await settle();
    expect(result.current.isOn("board")).toBe(true);
  });
});
