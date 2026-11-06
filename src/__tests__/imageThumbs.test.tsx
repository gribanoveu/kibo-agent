import { expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { ImageThumbs } from "../components/ImageThumbs";
import type { ImagePart } from "../lib/images";

const picture: ImagePart = { mediaType: "image/png", data: "AA==", width: 40, height: 30 };

/// The viewer leaves the thread, so nothing done to the thread's box cuts it
/// or its Close.
test("a picture opens over the window, not inside the thread it was sent in", () => {
  render(
    <div className="thread">
      <ImageThumbs images={[picture]} />
    </div>,
  );
  fireEvent.click(screen.getByTitle("40×30 — click to open"));

  const backdrop = screen.getByRole("dialog").parentElement!;
  expect(backdrop.closest(".thread")).toBeNull();
  expect(backdrop.parentElement).toBe(document.body);

  fireEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("a picture in the box is taken off, not opened, by its cross", () => {
  const removed: number[] = [];
  render(<ImageThumbs images={[picture, { ...picture, width: 20 }]} onRemove={(index) => removed.push(index)} />);
  fireEvent.click(screen.getByRole("button", { name: "Remove image 2" }));
  expect(removed).toEqual([1]);
});
