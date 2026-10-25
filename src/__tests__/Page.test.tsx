import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Store } from "lucide-react";
import { Page, PageSheet } from "../components/Page";

const sheet = (onClose: () => void) => (
  <PageSheet icon={Store} title="Thing" label="One thing" onClose={onClose}>
    <p>about it</p>
  </PageSheet>
);

describe("a page", () => {
  test("has its title and bar", () => {
    render(
      <Page title="Things" bar={<button type="button">Refresh</button>}>
        <p>listed</p>
      </Page>,
    );
    expect(screen.getByRole("region", { name: "Things" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Things" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Refresh" })).toBeTruthy();
  });

  test("lays its sheet over itself, and the sheet closes from its heading or Escape", () => {
    let closed = 0;
    render(
      <Page title="Things" sheet={sheet(() => closed++)}>
        <p>listed</p>
      </Page>,
    );
    // Inside the page, not beside it: the page keeps its width.
    expect(screen.getByRole("region", { name: "Things" }).contains(screen.getByRole("complementary", { name: "One thing" }))).toBe(true);
    expect(screen.getByRole("complementary", { name: "One thing" }).textContent).toContain("about it");
    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    expect(closed).toBe(1);
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(closed).toBe(2);
  });

  test("leaves Escape to a text field, and to a dialog or menu open over it", () => {
    let closed = 0;
    const { rerender } = render(
      <Page title="Things" sheet={sheet(() => closed++)}>
        <input aria-label="Name" />
      </Page>,
    );
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Name" }), { key: "Escape", code: "Escape" });
    expect(closed).toBe(0);
    rerender(
      <Page title="Things" sheet={sheet(() => closed++)}>
        <div role="dialog" aria-label="Settings" />
      </Page>,
    );
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(closed).toBe(0);
  });

  test("closes on a click outside it, but not on a click in it, on what opens it, or with a dialog over it", () => {
    let closed = 0;
    const { rerender } = render(
      <Page title="Things" sheet={sheet(() => closed++)}>
        <p>listed</p>
        <button type="button" data-sheet-opener>
          Open another
        </button>
      </Page>,
    );
    fireEvent.pointerDown(screen.getByText("about it"));
    fireEvent.pointerDown(screen.getByRole("button", { name: "Open another" }));
    expect(closed).toBe(0);
    fireEvent.pointerDown(screen.getByText("listed"));
    expect(closed).toBe(1);
    rerender(
      <Page title="Things" sheet={sheet(() => closed++)}>
        <p>listed</p>
        <div role="dialog" aria-label="Sure?" />
      </Page>,
    );
    fireEvent.pointerDown(screen.getByText("listed"));
    expect(closed).toBe(1);
  });
});
