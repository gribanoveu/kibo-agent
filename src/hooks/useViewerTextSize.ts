import { useEffect, useState } from "react";
import { setViewerTextScale, viewerTextScale } from "../lib/window";

/** The Markdown viewer's text size, as a factor on the type scale (`.chat-text`, src/styles/tokens.css). */
export const TEXT_SCALE = { min: 0.7, max: 2, step: 0.1, usual: 1.1 } as const;

/** One step up (`1`) or down (`-1`), held within the range and rounded off the float error. */
export const stepScale = (scale: number, direction: 1 | -1) =>
  Math.round(Math.min(TEXT_SCALE.max, Math.max(TEXT_SCALE.min, scale + direction * TEXT_SCALE.step)) * 100) / 100;

/**
 * Every viewer's, kept by the backend for the next (`commands/viewer.rs` says
 * why not in browser storage); not the chat's size, which Settings sets.
 * `scale` is `null` until the stored one is read, so the page is not drawn at
 * one size and then jumps to another.
 */
export function useViewerTextSize() {
  const [scale, setScale] = useState<number | null>(null);

  useEffect(() => {
    // A size that cannot be read is the usual one.
    viewerTextScale().then(
      (stored) => setScale(stored ?? TEXT_SCALE.usual),
      () => setScale(TEXT_SCALE.usual),
    );
  }, []);

  useEffect(() => {
    // One that cannot be written is only lost for the next viewer.
    if (scale !== null) setViewerTextScale(scale).catch(() => {});
  }, [scale]);

  const step = (direction: 1 | -1) => setScale((s) => stepScale(s ?? TEXT_SCALE.usual, direction));
  return {
    scale,
    larger: () => step(1),
    smaller: () => step(-1),
    reset: () => setScale(TEXT_SCALE.usual),
  };
}
