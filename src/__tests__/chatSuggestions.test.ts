import { describe, expect, test } from "bun:test";
import { greetingsAt, pickSuggestions } from "../lib/chatSuggestions";

// The empty chat's greeting: one of the hour's, picked without repeats.

const SET = ["one", "two", "three", "four", "five"];

describe("pickSuggestions", () => {
  test("two different ones from the set", () => {
    for (let run = 0; run < 50; run++) {
      const [a, b] = pickSuggestions(SET, 2);
      expect(a).not.toBe(b);
      expect(SET).toContain(a);
      expect(SET).toContain(b);
    }
  });

  test("any of the set can come up, the last included", () => {
    expect(pickSuggestions(SET, 2, () => 0)).toEqual([SET[0], SET[1]]);
    expect(pickSuggestions(SET, 1, () => 0.999)).toEqual([SET[SET.length - 1]]);
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
    const before = [...SET];
    pickSuggestions(SET, 2, () => 0.5);
    expect(SET).toEqual(before);
  });
});
