import { describe, expect, test } from "bun:test";
import { type AgentInfo, agentSpeed, tokensLive } from "../lib/agents";

// The helper card's live meter: the chat's own estimate (`liveSpeed`) applied
// to a run's streamed characters.

const NOW = 10_000_000;

const run = (over: Partial<AgentInfo> = {}): AgentInfo => ({
  id: 1,
  task: "t",
  state: { state: "running" },
  steps: [],
  tokens: { prompt: 1000, cached: 0, completion: 100 },
  answer: null,
  progress: { streamSince: null, streamChars: 0, timed: { tokens: 0, ms: 0, chars: 0 } },
  ...over,
});

const streaming = (chars: number, msAgo: number, timed = { tokens: 0, ms: 0, chars: 0 }): Partial<AgentInfo> => ({
  progress: { streamSince: NOW - msAgo, streamChars: chars, timed },
});

describe("agentSpeed", () => {
  test("null before the round streams, and for its first second", () => {
    expect(agentSpeed(run(), NOW)).toBeNull();
    expect(agentSpeed(run(streaming(140, 999)), NOW)).toBeNull();
  });

  test("characters at the chat's guess of 3.5 per token, over the streaming time", () => {
    // 140 characters ≈ 40 tokens over 2 seconds.
    expect(agentSpeed(run(streaming(140, 2000)), NOW)).toBe(20);
  });

  test("the finished rounds' ratio replaces the guess", () => {
    // 400 characters for 100 tokens: 4 per token; 160 ≈ 40 tokens over 2 s.
    expect(agentSpeed(run(streaming(160, 2000, { tokens: 100, ms: 1000, chars: 400 })), NOW)).toBe(20);
  });
});

describe("tokensLive", () => {
  test("reported rounds plus the one under way, estimated", () => {
    expect(tokensLive(run())).toBe(1100);
    expect(tokensLive(run(streaming(160, 2000, { tokens: 100, ms: 1000, chars: 400 })))).toBe(1140);
  });
});