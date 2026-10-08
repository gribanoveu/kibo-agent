// A chat's name animates when the model renames it, not when the list first draws it.
import { test, expect } from "bun:test";
import { render, screen } from "@testing-library/react";
import { ChangingText } from "../components/ChangingText";

test("eases in a changed text, not the first one", () => {
  const { rerender } = render(<ChangingText text="why is a token dropped?" />);
  expect(screen.getByText("why is a token dropped?").className).toBe("");

  rerender(<ChangingText text="Lost token" />);
  expect(screen.getByText("Lost token").className).toBe("changing-text");
});
