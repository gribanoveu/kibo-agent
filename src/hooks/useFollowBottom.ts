import { useCallback } from "react";
import { useStickToBottom } from "use-stick-to-bottom";

/**
 * Keeps a scrolling box at its end while it grows — until the user scrolls
 * away from the end; scrolling back to it picks it up again.
 *
 * Delegates to `use-stick-to-bottom` (StackBlitz), written for streaming AI
 * chat: it knows the writes its own animation makes and reads the direction
 * of every other scroll, so a late WebKit commit of our own write is never
 * taken for the user's, and a drag or a lone scrollbar click lets go at once.
 * No time windows to guess by, and no `overflow-anchor` — WebKit has none.
 *
 * `scrollRef` goes on the scrolling box, `contentRef` on the one child that
 * holds everything inside it. The content is what is observed, never the box:
 * the box's own height changes whenever a line is typed into the message box,
 * and sticking on that shoves what is being read up by a line per line typed.
 *
 * The pin is instant, as it was before the library: `resize`/`initial` keep
 * the growing answer at the end without a spring, and `scrollToBottom` passes
 * "instant" explicitly — the library's default for a bare call is its spring.
 *
 * The wheel and the scroll keys let go through listeners of our own, before
 * any scroll event: the library's wheel path finds the scrolling element by
 * its computed `overflow` shorthand, which the thread's `overflow-x: hidden`
 * turns into "hidden auto" — past the library's check — and it has no keyboard
 * path at all. Left to the scroll event alone, a let go waits a millisecond
 * while the instant pin keeps writing, and the wheel fights the stream.
 *
 * `isAtBottom` is the library's own flag (true within its 70px of the end):
 * what the back-to-bottom button shows by.
 */
export function useFollowBottom() {
  const {
    scrollRef: stickRef,
    contentRef,
    scrollToBottom: stick,
    stopScroll,
    isAtBottom,
  } = useStickToBottom({
    resize: "instant",
    initial: "instant",
  });

  const scrollRef = useCallback(
    (el: HTMLElement | null) => {
      stickRef(el);
      if (!el) return;
      // Only where there is something to scroll up to: a wheel turned up on a
      // thread shorter than its box moves nothing, and a let go there would
      // stop the following with no scroll able to pick it back up.
      const letGo = () => {
        if (el.scrollHeight > el.clientHeight) stopScroll();
      };
      const onWheel = (e: WheelEvent) => {
        if (e.deltaY < 0) letGo();
      };
      // These keys scroll the box natively; the app answers to no shortcut
      // here — the hook only notices them to let go of the end.
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "PageUp" || e.key === "ArrowUp" || e.key === "Home") letGo();
      };
      el.addEventListener("wheel", onWheel, { passive: true });
      el.addEventListener("keydown", onKey);
      return () => {
        el.removeEventListener("wheel", onWheel);
        el.removeEventListener("keydown", onKey);
        // React runs a returned cleanup instead of calling the ref with null.
        // Nulling the library's current makes its handlers ignore this box;
        // their physical removal the library leaves to the detached element's
        // garbage — its own unmount does the same.
        stickRef(null);
      };
    },
    [stickRef, stopScroll],
  );

  /** Back to the end, following again — for when the user just sent something. */
  const scrollToBottom = useCallback(() => {
    void stick("instant");
  }, [stick]);

  return { scrollRef, contentRef, scrollToBottom, isAtBottom };
}