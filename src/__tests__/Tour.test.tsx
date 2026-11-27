import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Tour } from "../components/Tour";
import { placeCard, placeCentred } from "../lib/tour";
import { IS_MAC, shortcutText } from "../lib/shortcuts";
import { useShortcuts } from "../hooks/useShortcuts";

const rect = (top: number, left: number, width: number, height: number) =>
  ({ top, left, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;

/** A window with a sidebar in it, as the app's has. */
const sidebar = () => {
  const el = document.createElement("div");
  el.setAttribute("data-tour", "sidebar");
  el.getBoundingClientRect = () => rect(10, 20, 100, 40);
  document.body.append(el);
  return el;
};

afterEach(() => {
  document.body.innerHTML = "";
});

describe("where the card goes", () => {
  const viewport = { width: 1000, height: 800 };
  const card = { width: 300, height: 120 };

  test("under the thing it points at, in the middle of it", () => {
    expect(placeCard({ top: 100, left: 200, width: 80, height: 30 }, card, viewport)).toEqual({ top: 142, left: 90 });
  });

  test("above it when the window's bottom is too close", () => {
    expect(placeCard({ top: 700, left: 200, width: 80, height: 30 }, card, viewport)).toEqual({ top: 568, left: 90 });
  });

  test("at the bottom when neither side has the room", () => {
    expect(placeCard({ top: 400, left: 200, width: 80, height: 30 }, { width: 300, height: 700 }, viewport).top).toBe(88);
  });

  test("never past the window's sides", () => {
    expect(placeCard({ top: 100, left: 0, width: 10, height: 10 }, card, viewport).left).toBe(12);
    expect(placeCard({ top: 100, left: 900, width: 100, height: 10 }, card, viewport).left).toBe(688);
  });

  test("the middle of the window when there is nothing to point at", () => {
    expect(placeCentred(card, viewport)).toEqual({ top: 340, left: 350 });
  });
});

describe("the tour", () => {
  test("rings what the step names, and says what it is", () => {
    sidebar();
    const { container } = render(<Tour onDone={() => {}} />);

    expect(screen.getByText("Your chats")).toBeTruthy();
    const ring = container.querySelector(".tour-ring") as HTMLElement;
    // The sidebar's box, a few pixels of air around it.
    expect([ring.style.top, ring.style.left, ring.style.width, ring.style.height]).toEqual(["4px", "14px", "112px", "52px"]);

    const card = container.querySelector(".tour-card") as HTMLElement;
    expect([card.style.top, card.style.left]).toEqual(["62px", "12px"]);
    // Everything the ring does not leave in the light takes no press.
    expect(container.querySelector(".tour-catch")).toBeTruthy();
    expect(container.querySelector(".tour-catch-dim")).toBeNull();
  });

  test("steps forward, and back to the one before", () => {
    render(<Tour onDone={() => {}} />);
    expect((screen.getByRole("button", { name: "Back" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("The panels")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByText("Your chats")).toBeTruthy();
  });

  test("the last step finishes it", () => {
    const onDone = mock(() => {});
    render(<Tour onDone={onDone} />);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("The message box")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  test("the panels step lists what the menu holds, with their keys", () => {
    render(<Tour onDone={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));

    for (const label of ["Overview", "Changes", "Plan", "Files", "Rules", "Skills", "MCP", "Hooks", "Terminal"]) {
      expect(screen.getByText(label)).toBeTruthy();
    }
    expect(screen.getByText(shortcutText("terminal"))).toBeTruthy();
  });

  test("the window's keys are held while it is up", () => {
    const spy = mock(() => {});
    const press = () =>
      fireEvent.keyDown(window, { code: "Digit1", key: "1", ctrlKey: !IS_MAC, metaKey: IS_MAC });
    function Keys() {
      useShortcuts({ overview: spy });
      return null;
    }

    const tree = render(<Keys />);
    press();
    expect(spy).toHaveBeenCalledTimes(1);

    tree.rerender(
      <>
        <Keys />
        <Tour onDone={() => {}} />
      </>,
    );
    press();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  test("Escape leaves it, and so does the cross", () => {
    const onDone = mock(() => {});
    render(<Tour onDone={onDone} />);
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    expect(onDone).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Leave the tour" }));
    expect(onDone).toHaveBeenCalledTimes(2);
  });

  test("with nothing to point at it speaks from the middle", () => {
    const { container } = render(<Tour onDone={() => {}} />);
    expect(container.querySelector(".tour-ring")).toBeNull();
    // No ring to cast the dim, so the cover holds it — and still takes the presses.
    expect(container.querySelector(".tour-catch-dim")).toBeTruthy();
    const card = container.querySelector(".tour-card") as HTMLElement;
    expect(Number.parseFloat(card.style.top)).toBeGreaterThan(0);
    expect(screen.getByText("Your chats")).toBeTruthy();
  });
});
