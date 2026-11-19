import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { InfoTip } from "../components/InfoTip";

// The reusable info icon: hover or focus shows the tooltip, a click pins it,
// Escape or a click outside closes it — the app's own, not the platform's.

const tip = () => render(<InfoTip text="What the number means." />);
const button = () => screen.getByRole("button", { name: "More info" });
const shown = () => screen.queryByRole("tooltip");

describe("InfoTip", () => {
  test("nothing until asked; hover shows it, leaving hides it", () => {
    tip();
    expect(shown()).toBeNull();
    fireEvent.pointerEnter(button());
    expect(shown()?.textContent).toBe("What the number means.");
    fireEvent.pointerLeave(button());
    expect(shown()).toBeNull();
  });

  test("focus shows it, blur hides it", () => {
    tip();
    fireEvent.focus(button());
    expect(shown()).toBeTruthy();
    fireEvent.blur(button());
    expect(shown()).toBeNull();
  });

  test("a click pins it open past the pointer leaving; another click closes it", () => {
    tip();
    fireEvent.click(button());
    fireEvent.pointerLeave(button());
    expect(shown()).toBeTruthy();
    fireEvent.click(button());
    expect(shown()).toBeNull();
  });

  test("Escape closes the pinned one", () => {
    tip();
    fireEvent.click(button());
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(shown()).toBeNull();
  });

  /// A mouse click focuses the button before it clicks: the focus's show
  /// must not keep the tooltip open past the closing click or Escape.
  test("focused by the click itself, a second click and Escape still close it", () => {
    tip();
    fireEvent.focus(button());
    fireEvent.click(button());
    expect(shown()).toBeTruthy();
    fireEvent.click(button());
    expect(shown()).toBeNull();
    // Hovering over the pinned one: Escape lets go of both shows.
    fireEvent.click(button());
    fireEvent.pointerEnter(button());
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(shown()).toBeNull();
    // And a click outside, with the hover's show still on.
    fireEvent.click(button());
    fireEvent.pointerEnter(button());
    fireEvent.pointerDown(document.body);
    expect(shown()).toBeNull();
  });

  test("a click outside closes the pinned one; a click on the icon does not", () => {
    tip();
    fireEvent.click(button());
    fireEvent.pointerDown(button());
    expect(shown()).toBeTruthy();
    fireEvent.pointerDown(document.body);
    expect(shown()).toBeNull();
  });

  test("a tooltip that would run past the edge of what clips it is moved back inside", () => {
    // happy-dom lays nothing out: the clipping box spans 100–400, the tooltip 40–260.
    const rect = Object.getOwnPropertyDescriptor(Element.prototype, "getBoundingClientRect")!;
    Element.prototype.getBoundingClientRect = function (this: Element) {
      const [left, right] = this.classList.contains("info-tip-pop") ? [40, 260] : this.classList.contains("clip") ? [100, 400] : [0, 0];
      return { left, right, top: 0, bottom: 0, width: right - left, height: 0, x: left, y: 0, toJSON: () => ({}) } as DOMRect;
    };
    try {
      render(
        <div className="clip" style={{ overflow: "auto" }}>
          <InfoTip text="What the number means." />
        </div>,
      );
      fireEvent.pointerEnter(button());
      expect(shown()!.style.transform).toBe("translateX(calc(-50% + 68px))");
    } finally {
      Object.defineProperty(Element.prototype, "getBoundingClientRect", rect);
    }
  });
});
