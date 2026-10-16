import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, render, screen } from "@testing-library/react";
import type { ProviderBalance as Balance } from "../lib/providerBalance";

// The chip beside the model (`Feature::ProviderBalance`): read when it
// appears, again when a turn ends and when a feature is switched — never while
// a turn runs.

let answer: Balance | null | Error;
let asked: unknown[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command !== "provider_balance") return Promise.resolve(null);
    asked.push(args?.id);
    return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer);
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

const { ProviderBalance } = await import("../components/ProviderBalance");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

const cny = (total: string, available = true): Balance => ({
  available,
  amounts: [{ currency: "CNY", total, granted: "0.00", toppedUp: total }],
});

beforeEach(() => {
  asked = [];
  answer = cny("110.00");
});

describe("ProviderBalance", () => {
  test("shows what is left, and is read again when the turn ends", async () => {
    const { rerender } = render(<ProviderBalance providerId="deepseek" running={false} />);
    await settle();
    expect(screen.getByText("¥110.00")).toBeTruthy();
    expect(asked).toEqual(["deepseek"]);

    answer = cny("109.50");
    rerender(<ProviderBalance providerId="deepseek" running={true} />);
    await settle();
    expect(asked).toHaveLength(1);
    expect(screen.getByText("¥110.00")).toBeTruthy();

    rerender(<ProviderBalance providerId="deepseek" running={false} />);
    await settle();
    expect(screen.getByText("¥109.50")).toBeTruthy();
  });

  test("nothing while the feature is off, and it appears once it is turned on", async () => {
    answer = null;
    const { container } = render(<ProviderBalance providerId="deepseek" running={false} />);
    await settle();
    expect(container.textContent).toBe("");

    answer = cny("5.00");
    act(() => listeners.forEach((changed) => changed()));
    await settle();
    expect(screen.getByText("¥5.00")).toBeTruthy();
  });

  test("an account that no longer covers a request is marked", async () => {
    answer = cny("0.00", false);
    render(<ProviderBalance providerId="deepseek" running={false} />);
    await settle();
    const chip = screen.getByText("¥0.00");
    expect(chip.className).toContain("spent");
    expect(chip.title).toContain("top up");
  });

  test("a failed read says so in the title, not as a number", async () => {
    answer = new Error("401 Unauthorized");
    render(<ProviderBalance providerId="deepseek" running={false} />);
    await settle();
    expect(screen.getByTitle("Balance: Error: 401 Unauthorized").textContent).toBe("—");
  });
});
