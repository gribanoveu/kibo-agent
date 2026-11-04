import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { ContextMeter } from "../components/ContextMeter";
import type { ChatUsage, ContextUsage } from "../lib/chat";

// The ring beside the send button, and the panel it opens: how full the
// window is, what the tokens go to, and folding the older part away.

const context = (over: Partial<ContextUsage> = {}): ContextUsage => ({
  instructions: 1_000,
  skills: 2_000,
  tools: 3_000,
  mcp: 6_000,
  conversation: 4_000,
  total: 16_000,
  limit: null,
  compactsAt: null,
  ...over,
});

const meter = (
  over: { context?: Partial<ContextUsage>; usage?: ChatUsage | null; running?: boolean; onCompact?: () => void } = {},
) =>
  render(
    <ContextMeter
      context={context(over.context)}
      usage={over.usage ?? null}
      running={over.running ?? false}
      onCompact={over.onCompact ?? (() => {})}
      up
    />,
  );

const openMeter = () => fireEvent.click(screen.getByRole("button", { name: /Context usage/ }));

describe("the context meter", () => {
  /// Without the window, a number of tokens says nothing: 8k is nothing on a
  /// 200k model and the end of the road on a 8k one.
  test("shows how much of the window is gone, once the window is known", () => {
    meter({ context: { limit: 200_000, compactsAt: 160_000 } });

    expect(screen.getByRole("button", { name: "Context usage: 8%" })).toBeTruthy();
    openMeter();
    expect(screen.getByText("16k of 200k tokens")).toBeTruthy();
  });

  test("opens upwards, and asks for a compaction from its panel", () => {
    let asked = 0;
    meter({ onCompact: () => (asked += 1) });

    openMeter();
    expect(screen.getByRole("dialog", { name: "Context" }).classList.contains("up")).toBe(true);
    fireEvent.click(screen.getByText("Compact now"));
    expect(asked).toBe(1);
    expect(screen.queryByRole("dialog", { name: "Context" })).toBeNull();
  });

  /// Mid-turn the history is the turn's, not the window's: shortening it from
  /// under a running request is not something to offer.
  test("but not while a turn is running", () => {
    meter({ running: true });
    openMeter();
    expect((screen.getByText("Compact now") as HTMLButtonElement).disabled).toBe(true);
  });

  /// Folding the conversation moves one of the numbers, and skills and MCP
  /// servers are the user's to switch off. Showing only the total hides which
  /// one a click would help with.
  test("says what the tokens are spent on, part by part", () => {
    meter({ context: { limit: 200_000, compactsAt: 160_000 } });

    openMeter();
    const text = screen.getByRole("dialog", { name: "Context" }).textContent ?? "";
    expect(text).toContain("Instructions and rules1k");
    expect(text).toContain("Built-in tools3k");
    expect(text).toContain("Skills2k");
    expect(text).toContain("MCP servers6k");
    expect(text).toContain("Conversation4k");
    expect(text).toContain("at 160k");
  });

  /// This is an estimate. When the provider has said what the last request
  /// really cost, that number belongs next to it rather than behind it.
  test("puts the provider's own count beside the estimate, with the cached share", () => {
    meter({ usage: { promptTokens: 9_500, completionTokens: 300, totalTokens: 9_800, cachedTokens: 9_000 } });

    openMeter();
    expect(screen.getByText("The last request actually cost 10k, 9k of it (95%) from the cache.")).toBeTruthy();
  });

  /// Zero is also what a provider that reports no cache sends: not a miss.
  test("says nothing was reported as cached when the provider gave no cached count", () => {
    meter({ usage: { promptTokens: 9_500, completionTokens: 300, totalTokens: 9_800, cachedTokens: 0 } });

    openMeter();
    expect(screen.getByText("The last request actually cost 10k, none of it reported as cached.")).toBeTruthy();
  });

  /// A session's spend runs into millions; 2040k is a number to count digits in.
  test("says millions as millions", () => {
    const spent = (n: number) => {
      const { unmount } = render(
        <ContextMeter context={context()} usage={null} running={false} onCompact={() => {}} spent={n} />,
      );
      openMeter();
      const shown = screen.getByText("Session tokens").nextElementSibling?.textContent;
      unmount();
      return shown;
    };
    expect(spent(999_000)).toBe("999k");
    expect(spent(999_600)).toBe("1M");
    expect(spent(2_040_000)).toBe("2.04M");
    expect(spent(6_087_000)).toBe("6.09M");
  });

  test("closes on Escape", () => {
    meter();
    openMeter();
    fireEvent.keyDown(document, { code: "Escape", key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Context" })).toBeNull();
  });

  /// No window is a number with no scale — not a ring filled against a guess.
  test("fills no ring without a known window, and says why", () => {
    const { container } = meter();

    expect(container.querySelector(".ctx-arc")).toBeNull();
    openMeter();
    expect(screen.getByText(/window is not known/)).toBeTruthy();
  });
});
