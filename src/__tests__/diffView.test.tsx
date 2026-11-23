import { describe, expect, test } from "bun:test";
import { act, fireEvent, render } from "@testing-library/react";
import { DiffView, rowAt, rowStarts } from "../components/DiffView";
import { fileRows } from "../lib/diffRows";

// The viewer's DiffView draws only the rows in view: a file thousands of lines
// long is a few dozen elements, placed where they would be.

const text = Array.from({ length: 2000 }, (_, i) => `line ${i + 1}`).join("\n") + "\n";
const drawn = () => [...document.querySelectorAll<HTMLElement>(".diff-row")];

describe("DiffView, virtual", () => {
  test("draws a margin of rows, not the whole file, in a box as tall as all of them", () => {
    render(<DiffView rows={fileRows(text, text, true)} virtual />);
    const rows = drawn();
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThan(200);
    const height = parseFloat(rows[0].style.height);
    expect(parseFloat(document.querySelector<HTMLElement>(".diff-rows")!.style.height)).toBe(2000 * height);
    expect(rows[1].style.top).toBe(`${height}px`);
  });

  test("scrolled, it draws the rows there, each at its own place", () => {
    render(<DiffView rows={fileRows(text, text, true)} virtual />);
    const height = parseFloat(drawn()[0].style.height);
    const box = document.querySelector<HTMLElement>(".diff-scroll")!;
    act(() => {
      box.scrollTop = 1000 * height;
      fireEvent.scroll(box);
    });
    const line1001 = drawn().find((row) => row.textContent?.endsWith("line 1001"));
    expect(line1001?.style.top).toBe(`${1000 * height}px`);
    // The start of the file is no longer drawn.
    expect(Math.min(...drawn().map((row) => parseFloat(row.style.top)))).toBeGreaterThan(800 * height);
    expect(drawn().length).toBeLessThan(200);
  });

  test("without `virtual` every row is drawn, as the chat's diffs are", () => {
    render(<DiffView rows={fileRows("a\nb\n", "a\nc\n", true)} />);
    expect(drawn()).toHaveLength(3);
    expect(drawn()[0].style.top).toBe("");
  });
});

describe("where wrapped rows start", () => {
  const rows = fileRows("", "abcdefghij\n\tx\n\nabcd\n", true);

  test("unwrapped, every row is one line", () => {
    expect(rowStarts(rows, null)).toEqual([0, 1, 2, 3, 4]);
  });

  test("wrapped, a row takes as many lines as its columns fill, a tab four of them, an empty one still one", () => {
    // 10 characters in 4 columns: 3 lines; "\tx" is 5 wide: 2; "": 1; "abcd": exactly 1.
    expect(rowStarts(rows, { cols: 4, headerCols: 4 })).toEqual([0, 3, 5, 6, 7]);
  });

  test("a hunk header wraps by its own width", () => {
    const hunk = fileRows("1\n2\n3\n4\n5\n6\n7\n8\n9\n", "1\n2\n3\n4\nfive\n6\n7\n8\n9\n", false);
    expect(hunk[0].kind).toBe("hunk");
    const header = "@@ -2,7 +2,7 @@".length;
    expect(rowStarts(hunk, { cols: 100, headerCols: 5 })[1]).toBe(Math.ceil(header / 5));
  });
});

describe("the row at a line", () => {
  // Rows starting at lines 0, 3, 5, 6; 7 lines in all.
  const starts = [0, 3, 5, 6, 7];

  test("is the last row starting at or before it — a row's own first line is that row", () => {
    expect([0, 1, 2, 3, 4, 5, 6].map((line) => rowAt(starts, line))).toEqual([0, 0, 0, 1, 1, 2, 3]);
    expect(rowAt(starts, 2.5)).toBe(0);
    expect(rowAt(starts, 99)).toBe(3);
  });
});

describe("DiffView, virtual and wrapped", () => {
  test("a long line takes as many rows of height as it wraps into, and the rows after it move down", () => {
    // What the layout would measure: 8px characters, a 400px view, 18px lines.
    const rect = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "getBoundingClientRect");
    const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
      const w = this.textContent === "0".repeat(64) ? 64 * 8 : 0;
      return { width: w, height: 0, top: 0, left: 0, right: w, bottom: 0, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
    };
    Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 400 });
    try {
      // Two number columns of 3 digits, the sign and the padding leave 38 columns.
      const long = "x".repeat(100);
      render(<DiffView rows={fileRows("a\n", `${long}\nb\n`, true)} wrap virtual />);
      const rows = drawn();
      const tops = rows.map((row) => parseFloat(row.style.top));
      const heights = rows.map((row) => parseFloat(row.style.height));
      expect(heights).toEqual([18, 54, 18]); // "a" removed, the long line added in 3, "b"
      expect(tops).toEqual([0, 18, 72]);
      expect(parseFloat(document.querySelector<HTMLElement>(".diff-rows")!.style.height)).toBe(90);
    } finally {
      // The method lives on Element.prototype, not here: assigning it back
      // would leave an own property on HTMLElement.prototype that shadows any
      // later Element.prototype mock (InfoTip's, in Windows' file order). Put
      // back only what was own; otherwise remove the mock outright.
      if (rect) Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", rect);
      else delete HTMLElement.prototype.getBoundingClientRect;
      if (width) Object.defineProperty(HTMLElement.prototype, "clientWidth", width);
    }
    // No own getBoundingClientRect left behind to shadow a later mock.
    expect(Object.getOwnPropertyDescriptor(HTMLElement.prototype, "getBoundingClientRect")).toBeUndefined();
  });
});
