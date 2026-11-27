import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Tour } from "../components/Tour";
import { placeCard, placeCentred } from "../lib/tour";
import { IS_MAC } from "../lib/shortcuts";
import { useShortcuts } from "../hooks/useShortcuts";

const rect = (top: number, left: number, width: number, height: number) =>
  ({ top, left, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;

/** A window with the thing the first step names in it, as the app has it. */
const anchor = (name = "mode") => {
  const el = document.createElement("div");
  el.setAttribute("data-tour", name);
  el.getBoundingClientRect = () => rect(10, 20, 100, 40);
  document.body.append(el);
  return el;
};

const next = () => fireEvent.click(screen.getByRole("button", { name: "Next" }));
const done = () => screen.queryByRole("button", { name: "Done" });

afterEach(() => {
  document.body.innerHTML = "";
});

/** The observers the tour opens on what it points at, so a test can say it resized. */
let observers: Array<{ cb: () => void; targets: Set<Element> }> = [];
const realObserver = globalThis.ResizeObserver;

beforeEach(() => {
  observers = [];
  globalThis.ResizeObserver = class {
    targets = new Set<Element>();
    constructor(readonly cb: () => void) {
      observers.push(this as unknown as { cb: () => void; targets: Set<Element> });
    }
    observe(el: Element) {
      this.targets.add(el);
    }
    unobserve(el: Element) {
      this.targets.delete(el);
    }
    disconnect() {
      this.targets.clear();
    }
  } as unknown as typeof ResizeObserver;
});

/** A resize of everything the tour is watching — nothing it is not watching hears it. */
const resized = () =>
  act(() =>
    observers
      .filter((one) => one.targets.size > 0)
      .forEach((one) => one.cb()),
  );

afterEach(() => {
  globalThis.ResizeObserver = realObserver;
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
    anchor();
    const { container } = render(<Tour onDone={() => {}} />);

    expect(screen.getByText("Agent and chat")).toBeTruthy();
    const ring = container.querySelector(".tour-ring") as HTMLElement;
    // The button's box, a few pixels of air around it.
    expect([ring.style.top, ring.style.left, ring.style.width, ring.style.height]).toEqual(["4px", "14px", "112px", "52px"]);

    const card = container.querySelector(".tour-card") as HTMLElement;
    expect([card.style.top, card.style.left]).toEqual(["62px", "12px"]);
    // Everything the ring does not leave in the light takes no press.
    expect(container.querySelector(".tour-catch")).toBeTruthy();
    expect(container.querySelector(".tour-catch-dim")).toBeNull();
  });

  test("the ring follows the thing it points at as it grows", () => {
    const el = anchor();
    const { container } = render(<Tour onDone={() => {}} />);
    const ring = () => container.querySelector(".tour-ring") as HTMLElement;
    expect(ring().style.width).toBe("112px");

    // A panel opening animates its width: same step, a bigger box.
    el.getBoundingClientRect = () => rect(10, 20, 300, 40);
    resized();
    expect(ring().style.width).toBe("312px");
  });

  test("a step that opens what it points at waits for it to arrive", async () => {
    const { container } = render(<Tour onDone={() => {}} />);
    expect(container.querySelector(".tour-ring")).toBeNull();

    // The Catalog is a page, opened for its step: it mounts a commit later.
    anchor();
    await waitFor(() => expect(container.querySelector(".tour-ring")).toBeTruthy(), { timeout: 1000 });
  });

  test("steps forward, and back to the one before", () => {
    render(<Tour onDone={() => {}} />);
    expect((screen.getByRole("button", { name: "Back" }) as HTMLButtonElement).disabled).toBe(true);

    next();
    expect(screen.getByText("The panels")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByText("Agent and chat")).toBeTruthy();
  });

  test("the last step finishes it", () => {
    const onDone = mock(() => {});
    render(<Tour onDone={onDone} />);
    // Every step but the last says Next; the last says Done.
    for (let at = 0; at < 10 && !done(); at += 1) next();
    expect(screen.getByText("A new chat")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  test("the panels step says what the menu is for, and lists nothing", () => {
    render(<Tour onDone={() => {}} />);
    next();

    expect(screen.getByText(/Everything about the folder and the work in it/)).toBeTruthy();
    // Read as a sentence, not as the menu itself: the menu cannot be opened
    // under the dim, and a list of nine is not what a tour is for.
    expect(screen.queryByText("Terminal")).toBeNull();
  });

  test("a panel the step points into is opened by a press on its own button", () => {
    const trigger = document.createElement("button");
    trigger.setAttribute("data-tour", "model");
    const pressed = mock(() => {});
    trigger.addEventListener("click", pressed);
    document.body.append(trigger);

    render(<Tour onDone={() => {}} />);
    // mode, panes, IDE, terminal, overview, folder, then the panel's own step.
    for (let at = 0; at < 6; at += 1) next();
    expect(screen.getByText("The model, and asking first")).toBeTruthy();
    expect(pressed).toHaveBeenCalledTimes(1);
  });

  test("the Catalog is opened for the step that shows it, and put away after", () => {
    const onOpen = mock((_id: string | null) => {});
    render(<Tour onDone={() => {}} onOpen={onOpen} />);
    expect(onOpen.mock.calls[0]).toEqual([null]);

    // The row in the sidebar first, then the page it opens.
    for (let at = 0; at < 10; at += 1) {
      if (screen.queryByText("The Catalog")) break;
      next();
    }
    expect(onOpen.mock.calls.at(-1)).toEqual([null]);

    next();
    expect(screen.getByText("Everything, in one place")).toBeTruthy();
    expect(onOpen.mock.calls.at(-1)).toEqual(["catalog"]);

    next();
    expect(screen.getByText("A new chat")).toBeTruthy();
    expect(onOpen.mock.calls.at(-1)).toEqual([null]);
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
    expect(screen.getByText("Agent and chat")).toBeTruthy();
  });
});
