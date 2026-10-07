// The window's own buttons: Windows' caption buttons at the right on Windows,
// the traffic lights everywhere else.
import { test, expect } from "bun:test";
import { render, screen } from "@testing-library/react";
import { WindowControls } from "../components/WindowControls";

test("on Windows draws caption buttons in Windows' order", () => {
  render(<WindowControls windows />);
  const labels = screen.getAllByRole("button").map((b) => b.getAttribute("aria-label"));
  expect(labels).toEqual(["Minimize", "Maximize", "Close"]);
  expect(document.querySelector(".window-controls.windows")).toBeTruthy();
  expect(document.querySelector(".dot")).toBeNull();
});

test("elsewhere draws the traffic lights, close first", () => {
  render(<WindowControls windows={false} />);
  const labels = screen.getAllByRole("button").map((b) => b.getAttribute("aria-label"));
  expect(labels).toEqual(["Close", "Minimize", "Maximize"]);
  expect(document.querySelector(".window-controls.windows")).toBeNull();
});
