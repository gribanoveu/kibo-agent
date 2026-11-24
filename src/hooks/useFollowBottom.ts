import { useCallback, useRef } from "react";

/**
 * Keeps a scrolling box at its end while it grows — until the user scrolls
 * away from the end; scrolling back to it picks it up again.
 *
 * The scroll happens in a ResizeObserver, which runs after layout and before
 * paint, so no frame is drawn with the new text below the edge. The content is
 * what is observed, never the box: the box's own height changes whenever a
 * line is typed into the message box, and sticking on that shoves what is
 * being read up by a line per line typed — with the answer at rest and nothing
 * else moving on screen. The end catches up with the next word of the answer
 * instead, and `scrollToBottom` puts it back when the user sends.
 *
 * `scrollRef` goes on the scrolling box, `contentRef` on the one child that
 * holds everything inside it.
 *
 * Letting go cannot wait for the scroll event alone. WebKit scrolls a box off
 * the main thread and reports it a frame or more later; a stream grows the
 * thread every frame, and the stick in between put the box back at the end
 * before the user's scroll was ever seen. So a wheel turned up (or a key that
 * scrolls up) lets go at once.
 *
 * Nor can a scroll event away from the end be taken for the user's on its
 * own: under load the scrolling thread's commit lags the stick, and the event
 * reads a position older than the last stick — or a commit clobbers the stick
 * itself. Both read as a scroll away while the answer streams. So an
 * ambiguous read holds the stick and waits for the next scroll event to
 * confirm it: a drag keeps sending them, a stale commit does not. A short
 * timeout re-sticks when nothing confirms.
 *
 * ponytail: a lone scrollbar-track click sends one event and is re-stuck —
 * it reads exactly like a stale commit, and letting the first event go is
 * the bug this confirmation fixes. The way past it is a scrollbar-intent
 * signal WebKit does not give; wheel, keys and a drag already let go.
 */
export function useFollowBottom() {
  const box = useRef<HTMLElement | null>(null);
  const following = useRef(true);
  // Where the last stick left the box, as the box reports it back.
  const stuckAt = useRef<number | null>(null);
  // The ambiguous read awaiting confirmation, and its re-stick timeout.
  const suspect = useRef<number | null>(null);
  const timer = useRef(0);

  const stick = useCallback(() => {
    const el = box.current;
    if (!following.current || !el || suspect.current !== null) return;
    el.scrollTop = el.scrollHeight;
    stuckAt.current = el.scrollTop;
  }, []);

  const settled = useCallback(() => {
    suspect.current = null;
    window.clearTimeout(timer.current);
  }, []);

  const scrollRef = useCallback(
    (el: HTMLElement | null) => {
      if (!el) return;
      box.current = el;
      // A pixel of slack: scrollTop is fractional on a Retina screen.
      const onScroll = () => {
        const top = el.scrollTop;
        if (el.scrollHeight - el.clientHeight - top < 2) {
          // A read at the end right after a wheel let go is the commit still
          // catching up, not the user back: stuckAt is null until a read away
          // from the end arms it again.
          if (stuckAt.current === null) return;
          settled();
          following.current = true;
          stuckAt.current = top;
          return;
        }
        // The first read away after a let go arms the end-check again: the
        // user has moved, so a later read at the end is them back.
        if (stuckAt.current === null) stuckAt.current = top;
        if (top === stuckAt.current) return;
        if (suspect.current !== null) {
          // A second read still up confirms the user; one back at the stick's
          // position was the commit catching up.
          if (top <= suspect.current + 2) {
            following.current = false;
            settled();
          } else {
            settled();
            stick();
          }
          return;
        }
        suspect.current = top;
        timer.current = window.setTimeout(() => {
          suspect.current = null;
          stick();
        }, 150);
      };
      // Only where there is something to scroll up to: a wheel turned up at
      // the top moves nothing, and letting go there would stop the thread
      // following with no scroll event ever able to pick it back up. The same
      // holds for an overflow of a pixel or two — the box is still at the end
      // within the slack, and a let go there is never picked back up.
      const letGo = () => {
        if (el.scrollTop <= 0 || el.scrollHeight - el.clientHeight < 2) return;
        following.current = false;
        stuckAt.current = null;
        settled();
      };
      const onWheel = (e: WheelEvent) => {
        if (e.deltaY < 0) letGo();
      };
      const onKey = (e: KeyboardEvent) => {
        if (e.key === "PageUp" || e.key === "ArrowUp" || e.key === "Home") letGo();
      };
      el.addEventListener("scroll", onScroll, { passive: true });
      el.addEventListener("wheel", onWheel, { passive: true });
      el.addEventListener("keydown", onKey);
      return () => {
        settled();
        el.removeEventListener("scroll", onScroll);
        el.removeEventListener("wheel", onWheel);
        el.removeEventListener("keydown", onKey);
        box.current = null;
      };
    },
    [stick, settled],
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
    settled();
    stick();
  }, [stick, settled]);

  return { scrollRef, contentRef, scrollToBottom };
}
