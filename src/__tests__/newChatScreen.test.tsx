import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen } from "@testing-library/react";

// A new chat's screen is the wordmark and nothing else: the folder, its branch
// and what the message goes out with are all in the card under it, so nothing
// here repeats them — with a folder open or without one.

mock.module("@tauri-apps/api/core", () => ({ invoke: () => Promise.resolve(null) }));

const { ChatEmptyState } = await import("../components/ChatEmptyState");

beforeEach(() => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("a new chat's screen", () => {
  test("is the wordmark alone — no title, no folder picker, no button", () => {
    render(<ChatEmptyState />);
    expect(document.querySelector(".chat-empty .kibo-brand")).toBeTruthy();
    expect(screen.queryByText("Nothing is open")).toBeNull();
    expect(screen.queryByText("kibo")).toBeNull();
    expect(document.querySelector(".chat-empty-recent")).toBeNull();
    // Choosing a folder is the composer's business, not this screen's.
    expect(screen.queryByRole("button")).toBeNull();
  });
});
