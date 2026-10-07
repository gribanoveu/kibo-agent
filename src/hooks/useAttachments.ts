import { useEffect, useRef, useState } from "react";
import { prepareImage, prepareImageFile, type ImagePart } from "../lib/chat";
import { onFileDrop } from "../lib/window";

/** Past this many a message takes no more: each is up to a megabyte in the chat file. */
export const MAX_IMAGES = 10;

/** Something to attach: a name to blame it by, and how to get it sanitized. */
type Source = { name: string; prepare: () => Promise<ImagePart> };

/**
 * The pictures attached to the message being written, each sanitized by the
 * backend as it arrives — what the box shows is what the model gets.
 *
 * `enabled` is whether the model turns go to is set to see them; when it is
 * not, a picture is refused with a sentence saying where to change that.
 *
 * Files dropped anywhere on the window come here while the box is on screen:
 * a drop aimed at the conversation above it means the same thing.
 */
export function useAttachments(enabled: boolean) {
  const [images, setImages] = useState<ImagePart[]>([]);
  const [error, setError] = useState<string | null>(null);
  /** Files are being held over the window: the box says it takes them. */
  const [dragging, setDragging] = useState(false);
  // The list as of now, for the checks between awaits: state seen from inside
  // `add` is the render's, not what an earlier picture of the same paste added.
  const held = useRef<ImagePart[]>([]);
  const hold = (next: ImagePart[]) => {
    held.current = next;
    setImages(next);
  };

  const add = async (sources: Source[]) => {
    if (!enabled) {
      setError("This model is set to text only — turn images on in Settings → Models.");
      return;
    }
    setError(null);
    for (const source of sources) {
      if (held.current.length >= MAX_IMAGES) {
        setError(`Up to ${MAX_IMAGES} images in one message.`);
        break;
      }
      try {
        const part = await source.prepare();
        // Checked again: another paste may have filled the box meanwhile.
        if (held.current.length >= MAX_IMAGES) {
          setError(`Up to ${MAX_IMAGES} images in one message.`);
          break;
        }
        hold([...held.current, part]);
      } catch (e) {
        setError(`${source.name}: ${e}`);
      }
    }
  };

  /** A paste's files: pictures are taken in and kept out of the box as
      anything else; without one, the paste goes on as usual. */
  const paste = (files: File[], e: { preventDefault: () => void }) => {
    const pictures = files.filter((file) => file.type.startsWith("image/"));
    if (pictures.length === 0) return;
    e.preventDefault();
    void add(
      pictures.map((file) => ({
        name: file.name || "The image",
        prepare: async () => prepareImage(new Uint8Array(await file.arrayBuffer())),
      })),
    );
  };

  // Every dropped file goes to the backend, which reads its content: one that
  // is not a PNG or JPEG comes back as a sentence naming it.
  useEffect(
    () =>
      onFileDrop((drop) => {
        setDragging(drop.type === "over");
        if (drop.type === "drop") {
          void add(drop.paths.map((path) => ({ name: fileName(path), prepare: () => prepareImageFile(path) })));
        }
      }),
    // `add` reads `enabled` and refs only; a new one per render changes nothing else.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [enabled],
  );

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

  return { images, error, dragging, paste, remove, take };
}

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;
