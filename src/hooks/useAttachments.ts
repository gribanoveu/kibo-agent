import { useEffect, useRef, useState } from "react";
import { type ImagePart, prepareImage, prepareImageFile } from "../lib/images";
import { onFileDrop } from "../lib/window";

/** Past this many a message takes no more: each is up to a megabyte in the chat file. */
export const MAX_IMAGES = 10;

/** Something to attach: a name to blame it by, and how to get it sanitized. */
type Source = { name: string; prepare: () => Promise<ImagePart> };

/** A picture held, with the name of the file it came from — a Prototype chat keeps it by that name. */
export type Attached = { image: ImagePart; name: string };

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
  /** Pictures being read and sanitized: one placeholder each, where they land. */
  const [preparing, setPreparing] = useState<number[]>([]);
  const preparingSeq = useRef(0);
  /** Files are being held over the window: the box says it takes them. */
  const [dragging, setDragging] = useState(false);
  // The list as of now, for the checks between awaits: state seen from inside
  // `add` is the render's, not what an earlier picture of the same paste added.
  const held = useRef<Attached[]>([]);
  const hold = (next: Attached[]) => {
    held.current = next;
    setImages(next.map((one) => one.image));
  };

  const add = async (sources: Source[]) => {
    if (!enabled) {
      setError("This model is set to text only — turn images on in Settings → Models.");
      return;
    }
    if (held.current.length >= MAX_IMAGES) {
      setError(`Up to ${MAX_IMAGES} images in one message.`);
      return;
    }
    setError(null);
    // A placeholder per file from the first moment: the backend reads and
    // sanitizes each, and a big screenshot takes a moment to come back.
    const placeholders = sources.map(() => ++preparingSeq.current);
    setPreparing((ids) => [...ids, ...placeholders]);
    try {
      for (const [at, source] of sources.entries()) {
        try {
          if (held.current.length >= MAX_IMAGES) {
            setError(`Up to ${MAX_IMAGES} images in one message.`);
            break;
          }
          const part = await source.prepare();
          // Checked again: another paste may have filled the box meanwhile.
          if (held.current.length >= MAX_IMAGES) {
            setError(`Up to ${MAX_IMAGES} images in one message.`);
            break;
          }
          hold([...held.current, { image: part, name: source.name }]);
        } catch (e) {
          setError(`${source.name}: ${e}`);
        } finally {
          // Its picture has landed, or the error says why there is none.
          setPreparing((ids) => ids.filter((one) => one !== placeholders[at]));
        }
      }
    } finally {
      // What the limit stopped before reaching never gets a picture.
      setPreparing((ids) => ids.filter((one) => !placeholders.includes(one)));
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
  // is not a PNG or JPEG comes back as a sentence naming it. Files picked in
  // the platform's dialog come the same way — by path.
  const attach = (paths: string[]) => {
    if (paths.length === 0) return;
    void add(paths.map((path) => ({ name: fileName(path), prepare: () => prepareImageFile(path) })));
  };

  useEffect(
    () =>
      onFileDrop((drop) => {
        setDragging(drop.type === "over");
        if (drop.type === "drop") attach(drop.paths);
      }),
    // `attach` reads `enabled` and refs only; a new one per render changes nothing else.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [enabled],
  );

  const remove = (index: number) => {
    hold(held.current.filter((_, at) => at !== index));
    setError(null);
  };

  /** Empties the box, giving back what it held, with their names — for the message being sent. */
  const takeNamed = () => {
    const taken = held.current;
    hold([]);
    setError(null);
    return taken;
  };
  const take = () => takeNamed().map((one) => one.image);

  /** Pictures put back into the box — a note taken back before a round read
      it comes home with what was attached to it. `replace` puts them in place
      of what the box holds, as a draft does with its text. */
  const restore = (images: ImagePart[], replace = false) => {
    if (images.length === 0) return;
    if (!enabled) {
      setError("This model is set to text only — turn images on in Settings → Models.");
      return;
    }
    const room = replace ? MAX_IMAGES : MAX_IMAGES - held.current.length;
    if (room < images.length) setError(`Up to ${MAX_IMAGES} images in one message.`);
    if (room <= 0) return;
    const put = images.slice(0, room).map((image) => ({ image, name: "The image" }));
    hold(replace ? put : [...held.current, ...put]);
  };

  /** The user closed the note; the next picture says again if it fails. */
  const clearError = () => setError(null);

  return { images, error, dragging, preparing: preparing.length, paste, remove, take, takeNamed, restore, attach, clearError };
}

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;
