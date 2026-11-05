import { useEffect, useRef, useState } from "react";
import { pickImages } from "../lib/dialog";
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
 * `key` is the box's owner — one chat's unsent pictures do not travel to the
 * next. Files dropped anywhere on the window come here while the box is on
 * screen: a drop aimed at the conversation above it means the same thing.
 */
export function useAttachments(enabled: boolean, key?: string) {
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
  /** The box these pictures belong to; another one starts with nothing. */
  const box = useRef(key);
  /** Bumped when the box changes hands: a picture read for the chat before does not land in this one. */
  const epoch = useRef(0);
  /** Pictures still being read, so a message can wait for the whole of itself. */
  const inFlight = useRef(new Set<Promise<unknown>>());
  const hold = (next: Attached[]) => {
    held.current = next;
    setImages(next.map((one) => one.image));
  };

  useEffect(() => {
    if (box.current === key) return;
    box.current = key;
    epoch.current += 1;
    held.current = [];
    setImages([]);
    setPreparing([]);
    setError(null);
  }, [key]);

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
    const mine = epoch.current;
    try {
      for (const [at, source] of sources.entries()) {
        try {
          if (held.current.length >= MAX_IMAGES) {
            setError(`Up to ${MAX_IMAGES} images in one message.`);
            break;
          }
          const part = await source.prepare();
          // Another chat opened while it was read: the picture belongs to the
          // box it was attached in, and that one is not on screen any more.
          if (epoch.current !== mine) break;
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

  /** Reads `sources` and keeps the promise, so a message can wait for all of them. */
  const run = (sources: Source[]) => {
    const work = add(sources);
    inFlight.current.add(work);
    const done = () => inFlight.current.delete(work);
    void work.then(done, done);
  };

  /** A paste's files: pictures are taken in and kept out of the box as
      anything else; without one, the paste goes on as usual. */
  const paste = (files: File[], e: { preventDefault: () => void }) => {
    const pictures = files.filter((file) => file.type.startsWith("image/"));
    if (pictures.length === 0) return;
    e.preventDefault();
    run(
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
    run(paths.map((path) => ({ name: fileName(path), prepare: () => prepareImageFile(path) })));
  };

  /**
   * The platform's picture picker, with the user's sidebar and their idea of
   * where their screenshots live. A dialog that refuses is a sentence in the
   * box, not a click that does nothing.
   */
  const pick = async () => {
    try {
      attach(await pickImages());
    } catch (e) {
      setError(String(e));
    }
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

  /** Waits for every picture being read: a message goes whole, never with the half of it that landed. */
  const settle = async () => {
    while (inFlight.current.size > 0) await Promise.all([...inFlight.current]);
  };

  /** Pictures put back into the box — a note taken back before a round read
      it comes home with what was attached to it. `replace` puts them in place
      of what the box holds, as a draft does with its text — and with no
      pictures it empties the box. What does not fit is said, never dropped
      without a word: the message it came from has already been cleared. */
  const restore = (images: ImagePart[], replace = false) => {
    if (images.length === 0) {
      if (!replace) return;
      hold([]);
      setError(null);
      return;
    }
    if (!enabled) {
      setError("This model is set to text only — turn images on in Settings → Models to attach the pictures that came back with the message.");
      return;
    }
    const room = replace ? MAX_IMAGES : MAX_IMAGES - held.current.length;
    const put = images.slice(0, Math.max(room, 0)).map((image) => ({ image, name: "The image" }));
    const dropped = images.length - put.length;
    if (dropped > 0) setError(`${dropped === 1 ? "One picture" : `${dropped} pictures`} did not fit: up to ${MAX_IMAGES} in one message.`);
    else setError(null);
    if (put.length === 0) return;
    hold(replace ? put : [...held.current, ...put]);
  };

  /** The user closed the note; the next picture says again if it fails. */
  const clearError = () => setError(null);

  return {
    enabled,
    images,
    error,
    dragging,
    preparing: preparing.length,
    paste,
    remove,
    take,
    takeNamed,
    restore,
    pick,
    settle,
    clearError,
  };
}

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;
