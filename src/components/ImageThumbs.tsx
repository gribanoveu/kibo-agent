import { useState } from "react";
import { createPortal } from "react-dom";
import { Loader2, X } from "lucide-react";
import { type ImagePart, imageUrl } from "../lib/images";
import { Modal } from "./Modal";
import "./ImageThumbs.css";

type Props = {
  images: ImagePart[];
  /** How many more are being sanitized right now. Each has a placeholder at
      the end of the row — where its picture will land — so the wait is seen
      rather than the row moving in later. */
  preparing?: number;
  /** Given, each picture can be taken off — the composer's; the transcript's cannot. */
  onRemove?: (index: number) => void;
  /** `end` lines the row up on the right, under a user's message. */
  align?: "start" | "end";
};

/** A row of pictures, each opened whole on click. */
export function ImageThumbs({ images, preparing = 0, onRemove, align = "start" }: Props) {
  const [open, setOpen] = useState<number | null>(null);
  if (images.length === 0 && preparing === 0) return null;
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
        {/* The pictures still being read and sanitized: as tall as a thumbnail,
            so the row keeps its height when they land. */}
        {Array.from({ length: preparing }, (_, at) => (
          <li
            key={`preparing-${at}`}
            className="image-thumb-pending"
            aria-label={`Preparing a picture ${at + 1}`}
            title="Preparing…"
          >
            <Loader2 size={16} className="image-thumb-spin" aria-hidden="true" />
          </li>
        ))}
      </ul>
      {/* Out of the thread and into the window, so nothing the thread does to
          its own box can cut the viewer or its Close button. The same escape a
          diagram takes (Markdown.tsx). */}
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
