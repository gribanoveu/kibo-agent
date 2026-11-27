import { useCallback, useRef } from "react";
import { useStickToBottom } from "use-stick-to-bottom";

/** The library's own "at the end" zone: past it the button shows, so a pixel
 *  of drag does not make it flicker. */
const AT_END_PX = 70;
/** A pixel of slack: `scrollTop` is fractional on a Retina screen. */
const SLACK = 2;
/** How long a position we wrote still reads as ours. Six hundred milliseconds
 *  is tens of frames of streaming — far past any commit lag seen under load. */
const LOG_MS = 600;
/** How many of our writes the log keeps, as a ceiling on memory: the age window
 *  is what bounds it at a browser's frame rate (a few dozen), and the count only
 *  matters where frames come faster than they should. */
const LOG_MAX = 4096;
/** Frames of sampling past the last animation the sampler knows of: the
 *  library starts its own a frame or two after the growth that causes it. */
const SAMPLE_GRACE = 4;

export type FollowBottom = {
  /** Goes on the scrolling box. */
  scrollRef: (el: HTMLElement | null) => (() => void) | undefined;
  /** Goes on the one child that holds everything inside it. */
  contentRef: (el: HTMLElement | null) => void;
  /** Back to the end and following again: a send, the button. `instant` is for
   *  a chat being opened rather than read through. */
  scrollToBottom: (instant?: boolean) => void;
  /** For `useSyncExternalStore` — the flag lives outside React's state, so the
   *  panel around the button does not re-render on a scroll. */
  subscribeAtBottom: (onChange: () => void) => () => void;
  getAtBottom: () => boolean;
};

/**
 * Keeps a scrolling box at its end while it grows — until the user scrolls
 * away from the end; scrolling back to it picks it up again.
 *
 * Delegates to `use-stick-to-bottom` (StackBlitz), written for streaming AI
 * chat: it knows the writes its own animation makes and reads the direction
 * of every other scroll, so a drag or a lone scrollbar click lets go at once.
 * No time windows to pick a let go by, and no `overflow-anchor` — WebKit has
 * none.
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
 * back to the end is a glide the wheel can cancel. Opening a chat is the one
 * exception: it asks for `instant`, because a glide through a long history on
 * every chat switch reads as a lag, not a delight.
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
 * Two reads of our own would otherwise be taken for the reader's, and both
 * stop the following with the button hidden, the box no further from its end
 * than before:
 *
 * - the clamp of a box that grew. The composer shrank after a send, the window
 *   got taller, a panel closed: `scrollTop` is pulled back a step with the
 *   content unchanged, and the library observes the content alone — it sees no
 *   resize to explain the step, and reads it as scrolling up.
 * - a commit that lands a frame or two behind our latest write. WebKit hands
 *   the position over late, so the read reports a position the spring left a
 *   moment ago: one this hook wrote, not one the reader moved to.
 *
 * Both are recognized here — a read at the end, or at a position the spring
 * wrote (`writes`, sampled from the library's own record a frame at a time and
 * dropped with every read, since a commit can only be reported late by a read
 * that follows it) — and claimed as `ignoreScrollToTop` before the library's
 * scroll listener runs, which is the library's own way of saying "this read is
 * mine". A read that is neither is the reader's: the library lets go, and
 * scrolling back to the end resumes.
 *
 * `atBottom` — the library's own 70px zone, read off the box — is what the
 * back-to-bottom button shows by. It goes when the reader asks for the end (a
 * send, a click) and comes back when a read of theirs says they are away from
 * it: the spring's own writes never put it back mid-glide. It is kept outside
 * React's state for the button alone: the panel around it is the whole
 * conversation, and a scroll that changes nothing but the button must not
 * re-render it.
 */
export function useFollowBottom(): FollowBottom {
  const {
    scrollRef: stickRef,
    contentRef: stickContentRef,
    scrollToBottom: stick,
    stopScroll,
    state,
  } = useStickToBottom({
    initial: "instant",
  });

  const box = useRef<HTMLElement | null>(null);
  const contentObserver = useRef<ResizeObserver | null>(null);
  // Where our spring left the box, and when: the reads that are ours.
  const writes = useRef<{ top: number; at: number }[]>([]);
  const sampling = useRef(0);
  const sampleLeft = useRef(0);
  // The button's flag and who to tell. Not React state: see the note above.
  const atBottom = useRef({ value: true, listeners: new Set<() => void>() });

  /** The button's flag, told to whoever listens — only when it flips. */
  const setAtBottom = useCallback((next: boolean) => {
    const flag = atBottom.current;
    if (flag.value === next) return;
    flag.value = next;
    for (const onChange of flag.listeners) onChange();
  }, []);

  const announce = useCallback(() => {
    const el = box.current;
    setAtBottom(!el || el.scrollHeight - el.clientHeight - el.scrollTop <= AT_END_PX);
  }, [setAtBottom]);

  const ours = useCallback((top: number) => {
    const now = performance.now();
    return writes.current.some((w) => now - w.at <= LOG_MS && Math.abs(w.top - top) < SLACK);
  }, []);

  // The library's own record of what its spring left, sampled a frame at a
  // time: a late commit reports one of these, and by the time the read arrives
  // the log is the only place that position is still known. Sampling runs
  // while the spring writes and a few frames past the growth that started it,
  // then stops — a thread at rest costs nothing.
  const sample = useCallback(() => {
    sampling.current = 0;
    const top = state.ignoreScrollToTop;
    const now = performance.now();
    const log = writes.current;
    while (log.length > 0 && now - log[0].at > LOG_MS) log.shift();
    if (top !== undefined && log[log.length - 1]?.top !== top) {
      log.push({ top, at: now });
      if (log.length > LOG_MAX) log.shift();
      // A write of ours may go without a scroll event of its own, and the end
      // is the button's to say: it goes when the thread lands there.
      const el = box.current;
      if (el && el.scrollHeight - el.clientHeight - el.scrollTop <= AT_END_PX) setAtBottom(true);
    }
    if (state.animation || sampleLeft.current > 0) {
      sampleLeft.current = Math.max(0, sampleLeft.current - 1);
      sampling.current = requestAnimationFrame(sample);
    }
  }, [state, setAtBottom]);

  const startSampling = useCallback(() => {
    sampleLeft.current = SAMPLE_GRACE;
    if (!sampling.current) sampling.current = requestAnimationFrame(sample);
  }, [sample]);

  const scrollRef = useCallback(
    (el: HTMLElement | null) => {
      if (!el) {
        stickRef(null);
        box.current = null;
        return;
      }
      box.current = el;
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
      const onScroll = () => {
        const top = el.scrollTop;
        const toEnd = el.scrollHeight - el.clientHeight - top;
        // At the end (within a pixel: the library's own target is a pixel above
        // the bottom), or at a position our spring wrote — the read is ours,
        // not the reader's. Said before the library's listener runs, this is
        // what makes it skip the read, exactly as it does its own writes.
        const mine = state.isAtBottom && (toEnd <= SLACK || ours(top));
        if (mine) {
          state.ignoreScrollToTop = top;
          if (toEnd <= AT_END_PX) setAtBottom(true);
          // The browser is at least where it just said: a read still on its way
          // reports a write above this one, and the ground behind this read is
          // no longer ours to claim — kept, the trail would widen into the
          // reader's way, the spring writing every frame.
          writes.current = writes.current.filter((w) => w.top >= top - SLACK);
        } else {
          // The reader's read: where the spring went is nothing to do with it.
          writes.current = [];
          setAtBottom(toEnd <= AT_END_PX);
        }
        startSampling();
      };
      // The scroll listener goes on first: the claim above holds only if it is
      // already made when the library's own listener looks at the read.
      el.addEventListener("scroll", onScroll, { passive: true });
      el.addEventListener("wheel", onWheel, { passive: true });
      el.addEventListener("keydown", onKey);
      stickRef(el);
      return () => {
        el.removeEventListener("scroll", onScroll);
        el.removeEventListener("wheel", onWheel);
        el.removeEventListener("keydown", onKey);
        // React runs a returned cleanup instead of calling the ref with null.
        // Nulling the library's current makes its handlers ignore this box;
        // their physical removal the library leaves to the detached element's
        // garbage — its own unmount does the same.
        stickRef(null);
        box.current = null;
        cancelAnimationFrame(sampling.current);
        sampling.current = 0;
      };
    },
    [stickRef, stopScroll, state, ours, setAtBottom, startSampling],
  );

  const contentRef = useCallback(
    (el: HTMLElement | null) => {
      contentObserver.current?.disconnect();
      contentObserver.current = null;
      stickContentRef(el);
      if (!el) return;
      // A growth moves the end under the reader and starts the spring; a
      // shrink can put them at it. While the thread follows, its own writes
      // and the reader's reads decide the button — a geometry read here would
      // put it back mid-glide — so only a thread that is not following is
      // read, where the end moving is the only word.
      contentObserver.current = new ResizeObserver(() => {
        if (!state.isAtBottom) announce();
        startSampling();
      });
      contentObserver.current.observe(el);
    },
    [stickContentRef, state, announce, startSampling],
  );

  /** Back to the end, following again — a send, or the button; instant for a
   *  chat being opened rather than read through. */
  const scrollToBottom = useCallback(
    (instant = false) => {
      void stick(instant ? "instant" : "smooth");
      // The reader has asked for the end: the button goes now, not when the
      // spring gets there.
      setAtBottom(true);
      startSampling();
    },
    [stick, setAtBottom, startSampling],
  );

  const subscribeAtBottom = useCallback((onChange: () => void) => {
    const flag = atBottom.current;
    flag.listeners.add(onChange);
    return () => {
      flag.listeners.delete(onChange);
    };
  }, []);

  const getAtBottom = useCallback(() => atBottom.current.value, []);

  return { scrollRef, contentRef, scrollToBottom, subscribeAtBottom, getAtBottom };
}
