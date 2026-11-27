import { describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { Titlebar } from "../components/Titlebar";

const slot = { current: null };

describe("the bar of a window a layer owns", () => {
  test("keeps its buttons in view and takes no press from them", () => {
    const { container, rerender } = render(
      <Titlebar leading={<button type="button">Sidebar</button>} slot={slot} held>
        Kibo Agent
      </Titlebar>,
    );
    expect(container.querySelector(".titlebar-leading")!.hasAttribute("inert")).toBe(true);
    expect(container.querySelector(".titlebar-slot")!.hasAttribute("inert")).toBe(true);

    rerender(
      <Titlebar leading={<button type="button">Sidebar</button>} slot={slot}>
        Kibo Agent
      </Titlebar>,
    );
    expect(container.querySelector(".titlebar-leading")!.hasAttribute("inert")).toBe(false);
    expect(container.querySelector(".titlebar-slot")!.hasAttribute("inert")).toBe(false);
  });

  test("the window's own controls are not held with them", () => {
    const { container } = render(
      <Titlebar slot={slot} held>
        Kibo Agent
      </Titlebar>,
    );
    // Whatever the bar draws for closing, minimising and zooming is the
    // window's, not the app's: it stays live.
    for (const control of container.querySelectorAll(".window-controls button")) {
      expect(control.closest("[inert]")).toBeNull();
    }
  });
});
