import { useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { type ImagePart, imageUrl } from "../lib/images";
import { Modal } from "./Modal";
import "./ImageThumbs.css";

type Props = {
  images: ImagePart[];
  /** Given, each picture can be taken off — the composer's; the transcript's cannot. */
  onRemove?: (index: number) => void;
  /** `end` lines the row up on the right, under a user's message. */
  align?: "start" | "end";
};

/** A row of pictures, each opened whole on click. */
export function ImageThumbs({ images, onRemove, align = "start" }: Props) {
  const [open, setOpen] = useState<number | null>(null);
  if (images.length === 0) return null;
  const shown = open === null ? null : (images[open] ?? null);
  return (
    <>
      <ul className={`image-thumbs${align === "end" ? " end" : ""}`} aria-label="Attached images">
        {images.map((image, index) => (
          <li key={index}>
            <button
              type="button"
              className="image-thumb"
              title={`${image.width}×${image.height} — click to open`}
              onClick={() => setOpen(index)}
            >
              <img src={imageUrl(image)} alt={`Image ${index + 1}, ${image.width}×${image.height}`} />
            </button>
            {onRemove && (
              <button
                type="button"
                className="image-thumb-remove"
                title="Remove"
                aria-label={`Remove image ${index + 1}`}
                onClick={() => onRemove(index)}
              >
                <X size={11} />
              </button>
            )}
          </li>
        ))}
      </ul>
      {/* Out of the thread and into the window: the thread fades its own bottom
          out with a mask, and a fixed panel drawn inside it is held to the
          thread's box and cut off with that fade — the viewer's Close button
          with it. The same escape a diagram takes (Markdown.tsx). */}
      {shown &&
        createPortal(
          <Modal title={`Image · ${shown.width}×${shown.height}`} open onClose={() => setOpen(null)} wide>
            <img className="image-full" src={imageUrl(shown)} alt="" />
          </Modal>,
          document.body,
        )}
    </>
  );
}
