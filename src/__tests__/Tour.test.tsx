import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Tour } from "../components/Tour";
import { placeCard, placeCentred } from "../lib/tour";

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
    const card = container.querySelector(".tour-card") as HTMLElement;
    expect(Number.parseFloat(card.style.top)).toBeGreaterThan(0);
    expect(screen.getByText("Your chats")).toBeTruthy();
  });
});
