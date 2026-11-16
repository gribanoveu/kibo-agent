import { describe, expect, test } from "bun:test";
import { type AgentInfo, agentSpeed, subagentMeter, tokensLive } from "../lib/agents";

// The Working panel's subagent line: the chat's own estimate (`liveSpeed`)
// applied to a run's streamed characters, summed over the running ones.

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

describe("subagentMeter", () => {
  test("nothing with none running — a finished one is not the line's", () => {
    expect(subagentMeter([], NOW)).toBeNull();
    expect(subagentMeter([run({ state: { state: "done" } }), run({ state: { state: "stopped" } })], NOW)).toBeNull();
  });

  test("one running: its speed and tokens, marked as an estimate while it streams", () => {
    expect(subagentMeter([run(streaming(140, 2000))], NOW)).toEqual({ count: 1, speed: 20, tokens: 1140, streaming: true });
  });

  test("several running: summed, with the count", () => {
    const meter = subagentMeter([run(streaming(140, 2000)), run({ id: 2, tokens: { prompt: 500, cached: 0, completion: 50 }, ...streaming(70, 2000) })], NOW);
    expect(meter).toEqual({ count: 2, speed: 30, tokens: 1710, streaming: true });
  });

  test("a run too young to estimate adds its tokens, not a speed", () => {
    const meter = subagentMeter([run(streaming(140, 2000)), run({ id: 2, ...streaming(70, 500) })], NOW);
    // The young one's 70 characters ≈ 20 tokens on top of its reported 1100.
    expect(meter).toEqual({ count: 2, speed: 20, tokens: 1140 + 1120, streaming: true });
  });

  test("none streaming: the reported tokens, no speed and no estimate mark", () => {
    const meter = subagentMeter([run(), run({ id: 2, state: { state: "running" } })], NOW);
    expect(meter).toEqual({ count: 2, speed: null, tokens: 2200, streaming: false });
  });
});