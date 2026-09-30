import { expect, test } from "bun:test";
import { describeUsage } from "../lib/chat";

// The line under the Tavily key: what it has spent, and what is left of it.

test("usage says what is spent and what is left, under the plan's name", () => {
  expect(describeUsage({ plan: "Researcher", used: 150, limit: 1000 })).toBe(
    "Researcher · 150 of 1,000 credits used this billing cycle, 850 left",
  );
  expect(describeUsage({ plan: null, used: 1200, limit: 1000 })).toBe(
    "1,200 of 1,000 credits used this billing cycle, 0 left",
  );
  expect(describeUsage({ plan: null, used: 7, limit: null })).toBe("7 credits used this billing cycle");
});
