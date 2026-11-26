import { useCallback, useRef } from "react";

/** How long a position we wrote stays cancellable: a commit lagging the stick
 *  by more than this reads as the user's. Six hundred milliseconds is tens of
 *  frames of streaming — far past any commit lag seen under load. */
const LOG_MS = 600;
/** How long after a let go its own writes may still arrive as scroll events.
 *  A frame or two; a user's return to the end comes later than this. */
const ECHO_MS = 150;
/** How many sticks the log keeps: a couple of seconds at a frame each. An echo
 *  is a frame old, and an answer at rest keeps its last stick however long the
 *  rest — so the log is capped by count, not pruned by age. */
const LOG_MAX = 128;
/** A pixel of slack: scrollTop is fractional on a Retina screen. */
const SLACK = 2;

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
 * A scroll event cannot be read as the user's while we write the box ourselves:
 * WebKit scrolls off the main thread and reports a position a frame or more
 * later, so under load an event reads a stick of ours — or the clamp a shrink
 * put on one — and taking that for the user stopped the thread mid-stream, or
 * froze the stick while a second read confirmed it. What we wrote is known, so
 * it is cancelled instead of guessed at: every stick logs the position it left,
 * and a read matching a recent entry is ours. A read at a position we never
 * wrote is the user's — a drag, a lone scrollbar click — and lets go at once,
 * with no second read and no pause in the sticking. A wheel turned up (or a key
 * that scrolls up) lets go before any scroll event, as it always did.
 *
 * While not following we write nothing, but the DOM does: a shrink clamps the
 * box to the new end. For a frame after a let go the echoes of our own writes
 * arrive too, so reads in that window are ignored; after it, a read at the end
 * picks the thread back up — the reader is at the end again, whether they
 * scrolled there or the clamp put them there.
 *
 * ponytail: the windows and the slack are the ceiling. A commit older than
 * LOG_MS reads as the user's and lets go; a read within SLACK of a position we
 * logged is cancelled even if the user put the box there (a drag sends many
 * reads, so one match costs nothing); a return to the end within ECHO_MS of a
 * let go is taken for an echo and waits for the next read. The way past them is
 * a scroll-intent signal WebKit does not give; wheel, keys, drags and scrollbar
 * clicks already let go on their first read.
 */
export function useFollowBottom() {
  const box = useRef<HTMLElement | null>(null);
  const following = useRef(true);
  // Where our sticks left the box, and when: the reads that are ours.
  const writes = useRef<{ top: number; at: number }[]>([]);
  // When we last let go: the echo window runs from it, not from a write — an
  // answer at rest lets go over a stick of a second ago, and its echo is still
  // an echo.
  const letGoAt = useRef(0);

  const stick = useCallback(() => {
    const el = box.current;
    if (!following.current || !el) return;
    el.scrollTop = el.scrollHeight;
    const log = writes.current;
    log.push({ top: el.scrollTop, at: performance.now() });
    if (log.length > LOG_MAX) log.shift();
  }, []);

  const scrollRef = useCallback(
    (el: HTMLElement | null) => {
      if (!el) return;
      box.current = el;
      const ours = (top: number, now: number, withinMs = Number.POSITIVE_INFINITY) =>
        writes.current.some((w) => now - w.at <= withinMs && Math.abs(w.top - top) < SLACK);
      const onScroll = () => {
        const top = el.scrollTop;
        const now = performance.now();
        const atEnd = el.scrollHeight - el.clientHeight - top < SLACK;
        if (following.current) {
          // Our own echo, or the clamp a shrink put on one: not the user.
          if (atEnd || ours(top, now, LOG_MS)) return;
          following.current = false;
          letGoAt.current = now;
          return;
        }
        // Not following: we write nothing, but the DOM's clamp and the echoes
        // of the let go still arrive. Within the echo window only our own
        // positions are cancelled — a return to a new end is the user's even
        // now; past it a read at the end is the reader at the end again, clamp
        // or return alike.
        if (now - letGoAt.current <= ECHO_MS && ours(top, now)) return;
        if (atEnd) {
          following.current = true;
          stick();
        }
      };
      // Only where there is something to scroll up to: a wheel turned up at
      // the top moves nothing, and letting go there would stop the thread
      // following with no scroll event ever able to pick it back up. The same
      // holds for an overflow of a pixel or two — the box is still at the end
      // within the slack, and a let go there is never picked back up.
      const letGo = () => {
        if (el.scrollTop <= 0 || el.scrollHeight - el.clientHeight < 2) return;
        following.current = false;
        letGoAt.current = performance.now();
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
        el.removeEventListener("scroll", onScroll);
        el.removeEventListener("wheel", onWheel);
        el.removeEventListener("keydown", onKey);
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
