import { expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Modal } from "../components/Modal";

test("a click on the backdrop closes, a selection dragged out of the dialog does not", () => {
  const onClose = mock(() => {});
  render(
    <Modal title="Insert" open onClose={onClose}>
      <textarea aria-label="Text" />
    </Modal>,
  );
  const backdrop = screen.getByRole("dialog").parentElement!;
  fireEvent.pointerDown(screen.getByRole("textbox", { name: "Text" }));
  fireEvent.click(backdrop);
  expect(onClose).not.toHaveBeenCalled();

  fireEvent.pointerDown(backdrop);
  fireEvent.click(backdrop);
  expect(onClose).toHaveBeenCalledTimes(1);
});
