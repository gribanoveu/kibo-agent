import { describe, expect, test } from "bun:test";
import { CHAT_SUGGESTIONS, KUBE_SUGGESTIONS, ROLE_SUGGESTIONS, greetingsAt, pickSuggestions } from "../lib/chatSuggestions";

// Chat mode's empty chat shows two of its role's set, different each time it opens.

describe("pickSuggestions", () => {
  test("the assistant's set has thirty, none twice", () => {
    expect(CHAT_SUGGESTIONS.length).toBe(30);
    expect(new Set(CHAT_SUGGESTIONS).size).toBe(30);
  });

  test("each role offers its own: Kubernetes asks about the cluster, not about Spring", () => {
    expect(ROLE_SUGGESTIONS.assistant).toBe(CHAT_SUGGESTIONS);
    expect(ROLE_SUGGESTIONS.kubernetes).toBe(KUBE_SUGGESTIONS);
    expect(new Set(KUBE_SUGGESTIONS).size).toBe(KUBE_SUGGESTIONS.length);
    expect(KUBE_SUGGESTIONS.some((s) => CHAT_SUGGESTIONS.includes(s))).toBe(false);
  });

  test("two different ones from the set", () => {
    for (let run = 0; run < 50; run++) {
      const [a, b] = pickSuggestions(KUBE_SUGGESTIONS, 2);
      expect(a).not.toBe(b);
      expect(KUBE_SUGGESTIONS).toContain(a);
      expect(KUBE_SUGGESTIONS).toContain(b);
    }
  });

  test("any of the set can come up, the last included", () => {
    expect(pickSuggestions(CHAT_SUGGESTIONS, 2, () => 0)).toEqual([CHAT_SUGGESTIONS[0], CHAT_SUGGESTIONS[1]]);
    expect(pickSuggestions(CHAT_SUGGESTIONS, 1, () => 0.999)).toEqual([CHAT_SUGGESTIONS[29]]);
  });

  test("a greeting fits the hour: no good morning at night, the general ones always", () => {
    expect(greetingsAt(9)).toContain("Good morning");
    expect(greetingsAt(14)).toContain("Good afternoon");
    expect(greetingsAt(20)).toContain("Good evening");
    for (const hour of [23, 0, 4]) expect(greetingsAt(hour)).toContain("Up late?");
    expect(greetingsAt(5)).toContain("Good morning");
    expect(greetingsAt(2)).not.toContain("Good morning");
    for (const hour of [0, 9, 14, 20]) expect(greetingsAt(hour)).toContain("How can I help?");
  });

  test("the set is left as it was", () => {
    const before = [...CHAT_SUGGESTIONS];
    pickSuggestions(CHAT_SUGGESTIONS, 2, () => 0.5);
    expect(CHAT_SUGGESTIONS).toEqual(before);
  });
});
