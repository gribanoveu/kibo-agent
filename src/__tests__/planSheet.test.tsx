import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { PlanSheet } from "../components/PlanSheet";

const PLAN = "# Fix the parser\n\n- read the grammar";

const sheet = (onClose: () => void, plan: string | null = PLAN) =>
  render(<PlanSheet plan={{ plan, checklist: [], onEdit: () => {}, locked: false }} onClose={onClose} />);

describe("the plan", () => {
  /// A sheet over the chat, not the column's pane: it is the app's sheet, with
  /// the plan drawn in it as the pane draws it.
  test("is read in a sheet, under the pane's own name", () => {
    sheet(() => {});
    const box = screen.getByRole("complementary", { name: "The plan for this chat" });
    expect(box.className).toContain("page-sheet");
    expect(screen.getByRole("heading", { name: "Plan" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Fix the parser" })).toBeTruthy();
  });

  test("closes from its heading and from Escape, as every sheet does", () => {
    let closed = 0;
    sheet(() => closed++);
    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    expect(closed).toBe(1);
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(closed).toBe(2);
  });

  test("with no plan yet, it is the panel's own empty one, with the way to write it", () => {
    sheet(() => {}, null);
    expect(screen.queryByRole("heading", { name: "Fix the parser" })).toBeNull();
    expect(screen.getByText("write your own")).toBeTruthy();
  });
});
