import { describe, expect, test } from "bun:test";
import { CHAT_SUGGESTIONS, pickSuggestions } from "../lib/chatSuggestions";

// Chat mode's empty chat shows two of the set, different each time it opens.

describe("pickSuggestions", () => {
  test("the set has thirty, none twice", () => {
    expect(CHAT_SUGGESTIONS.length).toBe(30);
    expect(new Set(CHAT_SUGGESTIONS).size).toBe(30);
  });

  test("two different ones from the set", () => {
    for (let run = 0; run < 50; run++) {
      const [a, b] = pickSuggestions(2);
      expect(a).not.toBe(b);
      expect(CHAT_SUGGESTIONS).toContain(a);
      expect(CHAT_SUGGESTIONS).toContain(b);
    }
  });

  test("any of the set can come up, the last included", () => {
    expect(pickSuggestions(2, () => 0)).toEqual([CHAT_SUGGESTIONS[0], CHAT_SUGGESTIONS[1]]);
    expect(pickSuggestions(1, () => 0.999)).toEqual([CHAT_SUGGESTIONS[29]]);
  });

  test("the set is left as it was", () => {
    const before = [...CHAT_SUGGESTIONS];
    pickSuggestions(2, () => 0.5);
    expect(CHAT_SUGGESTIONS).toEqual(before);
  });
});
