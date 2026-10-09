import { expect, test } from "bun:test";
import { stepScale, TEXT_SCALE } from "../hooks/useViewerTextSize";

test("a step moves by a tenth, without float error", () => {
  expect(stepScale(1.1, 1)).toBe(1.2);
  expect(stepScale(0.8, -1)).toBe(0.7);
});

test("a step stops at either end of the range", () => {
  expect(stepScale(TEXT_SCALE.max, 1)).toBe(TEXT_SCALE.max);
  expect(stepScale(TEXT_SCALE.min, -1)).toBe(TEXT_SCALE.min);
});
