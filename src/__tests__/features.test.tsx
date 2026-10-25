import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { FeatureView } from "../lib/features";

// The app's own features on the Catalog page (docs/33-features.md): a card
// beside the catalog's entries, turned on and off, its data deleted only
// while off and only after asking.

let board: FeatureView;
let calls: { command: string; args?: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "catalog_list") return Promise.resolve([]);
    if (command === "features_list") return Promise.resolve([structuredClone(board)]);
    if (command === "features_set") {
      board = { ...board, on: args!.on as boolean };
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
  board = {
    id: "board",
    name: "Board",
    description: "Cards for later",
    on: false,
    tools: [{ name: "board", description: "When the user asks you to remind them…" }],
    places: ["Assistant chat", "Agent", "Plan", "Ask"],
    data: "~/.kibo/board.json",
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
    expect(panel.getByText("Assistant chat, Agent, Plan, Ask")).toBeTruthy();
    expect(panel.getByText("When the user asks you to remind them…")).toBeTruthy();
    expect(panel.getByText("~/.kibo/board.json")).toBeTruthy();
  });

  test("+ turns it on, and the panel turns it off again", async () => {
    render(<CatalogPage openPane={() => {}} />);
    await settle();
    fireEvent.click(screen.getByLabelText("Turn on Board"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_set", args: { id: "board", on: true } });
    expect(screen.getByLabelText("On")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Board" }));
    fireEvent.click(screen.getByRole("button", { name: "Turn off" }));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "features_set", args: { id: "board", on: false } });
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
