import { useCallback, useRef } from "react";

/**
 * Keeps a scrolling box at its end while it grows — until the user scrolls
 * away from the end; scrolling back to it picks it up again.
 *
 * The scroll happens in a ResizeObserver, which runs after layout and before
 * paint, so no frame is drawn with the new text below the edge. The box is
 * observed too: a shorter window or a taller composer keeps the end in view.
 *
 * `scrollRef` goes on the scrolling box, `contentRef` on the one child that
 * holds everything inside it.
 *
 * Letting go cannot wait for the scroll event alone. WebKit scrolls a box off
 * the main thread and reports it a frame or more later; a stream grows the
 * thread every frame, and the stick in between put the box back at the end
 * before the user's scroll was ever seen. So a wheel turned up lets go at
 * once, and a scroll event that still reads the position the stick left says
 * nothing about the user.
 */
export function useFollowBottom() {
  const box = useRef<HTMLElement | null>(null);
  const following = useRef(true);
  // Where the last stick left the box, as the box reports it back.
  const stuckAt = useRef<number | null>(null);

  const stick = useCallback(() => {
    const el = box.current;
    if (!following.current || !el) return;
    el.scrollTop = el.scrollHeight;
    stuckAt.current = el.scrollTop;
  }, []);

  const scrollRef = useCallback(
    (el: HTMLElement | null) => {
      if (!el) return;
      box.current = el;
      // A pixel of slack: scrollTop is fractional on a Retina screen.
      const onScroll = () => {
        if (el.scrollTop === stuckAt.current) return;
        following.current = el.scrollHeight - el.clientHeight - el.scrollTop < 2;
      };
      const onWheel = (e: WheelEvent) => {
        if (e.deltaY < 0) following.current = false;
      };
      const resize = new ResizeObserver(stick);
      resize.observe(el);
      el.addEventListener("scroll", onScroll, { passive: true });
      el.addEventListener("wheel", onWheel, { passive: true });
      return () => {
        resize.disconnect();
        el.removeEventListener("scroll", onScroll);
        el.removeEventListener("wheel", onWheel);
        box.current = null;
      };
    },
    [stick],
  );

  const contentRef = useCallback(
    (el: HTMLElement | null) => {
      if (!el) return;
      const growth = new ResizeObserver(stick);
      growth.observe(el);
      return () => growth.disconnect();
    },
    [stick],
  );

  /** Back to the end, following again — for when the user just sent something. */
  const scrollToBottom = useCallback(() => {
    following.current = true;
    stick();
  }, [stick]);

  return { scrollRef, contentRef, scrollToBottom };
}
