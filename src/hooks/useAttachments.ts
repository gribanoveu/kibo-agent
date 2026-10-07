import { useRef, useState, type DragEvent } from "react";
import { prepareImage, type ImagePart } from "../lib/chat";

/** Past this many a message takes no more: each is up to a megabyte in the chat file. */
export const MAX_IMAGES = 10;

/**
 * The pictures attached to the message being written, each sanitized by the
 * backend as it arrives — what the box shows is what the model gets.
 *
 * `enabled` is whether the model turns go to is set to see them; when it is
 * not, a picture is refused with a sentence saying where to change that.
 */
export function useAttachments(enabled: boolean) {
  const [images, setImages] = useState<ImagePart[]>([]);
  const [error, setError] = useState<string | null>(null);
  // The list as of now, for the checks between awaits: state seen from inside
  // `add` is the render's, not what an earlier picture of the same paste added.
  const held = useRef<ImagePart[]>([]);
  const hold = (next: ImagePart[]) => {
    held.current = next;
    setImages(next);
  };

  const add = async (pictures: File[]) => {
    if (!enabled) {
      setError("This model is set to text only — turn images on in Settings → Models.");
      return;
    }
    setError(null);
    for (const file of pictures) {
      if (held.current.length >= MAX_IMAGES) {
        setError(`Up to ${MAX_IMAGES} images in one message.`);
        break;
      }
      try {
        const part = await prepareImage(new Uint8Array(await file.arrayBuffer()));
        // Checked again: another paste may have filled the box meanwhile.
        if (held.current.length >= MAX_IMAGES) {
          setError(`Up to ${MAX_IMAGES} images in one message.`);
          break;
        }
        hold([...held.current, part]);
      } catch (e) {
        setError(`${file.name || "The image"}: ${e}`);
      }
    }
  };

  const remove = (index: number) => {
    hold(held.current.filter((_, at) => at !== index));
    setError(null);
  };

  /** Empties the box, giving back what it held — for the message being sent. */
  const take = () => {
    const taken = held.current;
    hold([]);
    setError(null);
    return taken;
  };

  /** A paste's or a drop's files: pictures are taken in and kept out of the
      box as anything else; without one, the event goes on as usual. */
  const attach = (files: File[], e: { preventDefault: () => void }) => {
    const pictures = files.filter(isPicture);
    if (pictures.length === 0) return;
    e.preventDefault();
    void add(pictures);
  };

  /** Without it a drop is the webview's own, which opens the file in the window. */
  const dragOver = (e: DragEvent) => {
    if (e.dataTransfer.types.includes("Files")) e.preventDefault();
  };

  return { images, error, attach, dragOver, remove, take };
}

/** By the type the browser gives it — only to route it; the backend reads the content. */
const isPicture = (file: File) => file.type.startsWith("image/");
