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
 * The growth is chased by the library's spring, not snapped to: an instant
 * pin lands a frame after the ResizeObserver that saw the growth, so every
 * streamed word paints one frame below the edge and then jumps. The spring
 * writes every frame and glides after the answer instead. An explicit
 * `scrollToBottom` — a send, the button — rides the same spring, so the way
 * back to the end is a glide the wheel can cancel. Opening a chat (`initial`)
 * stays instant: a glide through a long history on every chat switch reads
 * as a lag, not a delight.
 *
 * The wheel and the scroll keys let go through listeners of our own, before
 * any scroll event: the library's wheel path finds the scrolling element by
 * its computed `overflow` shorthand, which the thread's `overflow-x: hidden`
 * turns into "hidden auto" — past the library's check — and it has no keyboard
 * path at all. Left to the scroll event alone, a let go waits a millisecond
 * while the pin keeps writing, and the wheel fights the stream. The wheel
 * only lets go when it is the thread's own: approval previews, diffs and tool
 * output scroll inside their own boxes, and their wheel events bubble up here
 * without the thread moving a pixel.
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
        // Up only, and only where the thread can move: a wheel turned up at
        // the very top scrolls nothing, and a let go there would stop the
        // following with no scroll able to pick it back up.
        if (e.deltaY >= 0 || el.scrollTop <= 0) return;
        // The wheel belongs to the innermost box it can scroll: a preview or
        // a diff with its own scrollbar takes it, and the thread must not let
        // go for a scroll that never moves it. At the inner box's own top the
        // wheel chains up to the thread — that one is ours — unless the box
        // keeps its overscroll to itself (contain/none): then the browser
        // chains nothing past it, and the wheel stays its own even there.
        let node = e.target as HTMLElement | null;
        while (node && node !== el) {
          const style = getComputedStyle(node);
          if (style.overflowY === "auto" || style.overflowY === "scroll") {
            const chains = style.overscrollBehaviorY !== "contain" && style.overscrollBehaviorY !== "none";
            if (node.scrollHeight > node.clientHeight && (node.scrollTop > 0 || !chains)) return;
          }
          node = node.parentElement;
        }
        letGo();
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
    void stick("smooth");
  }, [stick]);

  return { scrollRef, contentRef, scrollToBottom, isAtBottom };
}