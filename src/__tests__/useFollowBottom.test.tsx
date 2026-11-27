import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, fireEvent, render } from "@testing-library/react";
import { useFollowBottom } from "../hooks/useFollowBottom";
import { ScrollToBottomButton } from "../components/ScrollToBottomButton";

// The chat thread stays at its end while the answer grows, and lets go of it
// the moment the user scrolls up to read. The hook delegates to
// use-stick-to-bottom, which writes the scroll in animation frames and reads
// the direction of every scroll it did not write — so these tests let the
// frames run before reading the box, and the end is the library's own
// (scrollHeight - 1 - clientHeight).

// One entry per observed node, carrying the height the test last gave it:
// the library reads contentRect.height from the record, not the element.
let resized: { target: Element; cb: ResizeObserverCallback; height: number }[] = [];
const RealObserver = globalThis.ResizeObserver;
beforeEach(() => {
  resized = [];
  globalThis.ResizeObserver = class {
    constructor(private cb: ResizeObserverCallback) {}
    observe(target: Element) {
      resized.push({ target, cb: this.cb, height: 0 });
    }
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => {
  globalThis.ResizeObserver = RealObserver;
});

function Thread({ onReady }: { onReady: (api: ReturnType<typeof useFollowBottom>) => void }) {
  const api = useFollowBottom();
  onReady(api);
  return (
    // As the real thread: overflow-y auto with overflow-x hidden, so the
    // computed overflow shorthand is not a bare "auto" — the library's own
    // wheel path walks past it, and the hook's own listener must let go.
    <div ref={api.scrollRef} data-testid="box" style={{ overflowY: "auto", overflowX: "hidden" }}>
      <div ref={api.contentRef} data-testid="content" />
    </div>
  );
}

/** Let the library's frames and its 1ms scroll reads run. */
const settle = (ms = 80) => act(async () => void (await Bun.sleep(ms)));
/** Let the spring catch up: from far up it approaches the end over ~1.5s. */
const glide = () => settle(1600);

function setup(start = 1000) {
  let api!: ReturnType<typeof useFollowBottom>;
  const { getByTestId, unmount } = render(<Thread onReady={(a) => (api = a)} />);
  const box = getByTestId("box");
  const content = getByTestId("content");
  let height = start;
  const shown = 300;
  Object.defineProperty(box, "scrollHeight", { get: () => height });
  Object.defineProperty(box, "clientHeight", { get: () => shown });
  const fire = (target: Element) => {
    for (const entry of resized) {
      if (entry.target !== target) continue;
      entry.height = height;
      entry.cb([{ contentRect: { height } } as ResizeObserverEntry], {} as ResizeObserver);
    }
  };
  // The answer grows a word longer; the frames pin the box to the new end.
  const grow = async (by: number) => {
    height += by;
    act(() => fire(content));
    await settle();
  };
  // A tool card swapping its preview for a result: the thread shorter.
  const shrink = async (by: number) => {
    height -= by;
    act(() => fire(content));
    await settle();
  };
  const userScrollsTo = async (top: number) => {
    box.scrollTop = top;
    act(() => {
      box.dispatchEvent(new Event("scroll"));
    });
    await settle();
  };
  const wheelUp = async () => {
    act(() => {
      box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    });
    await settle();
  };
  // The library's end: a pixel above the very bottom.
  const end = () => height - 1 - shown;
  // A preview or a diff inside the thread, scrolling inside its own box.
  const nested = (top: number) => {
    const inner = document.createElement("div");
    inner.style.overflowY = "auto";
    Object.defineProperty(inner, "scrollHeight", { get: () => 500 });
    Object.defineProperty(inner, "clientHeight", { get: () => 100 });
    inner.scrollTop = top;
    content.appendChild(inner);
    return inner;
  };
  const wheelOn = async (target: Element, deltaY = -40) => {
    act(() => {
      target.dispatchEvent(new WheelEvent("wheel", { deltaY, bubbles: true }));
    });
    await settle();
  };
  return { box, grow, shrink, userScrollsTo, wheelUp, wheelOn, nested, end, unmount, api: () => api };
}

describe("following the end of the thread", () => {
  test("the first growth snaps to the end, later growth glides after it", async () => {
    const { box, grow, end } = setup();
    await grow(200); // the first growth takes the initial animation: instant
    expect(box.scrollTop).toBe(end());
    await grow(400); // the next takes the resize spring: mid-glide, not snapped
    expect(box.scrollTop).toBeGreaterThan(end() - 400);
    expect(box.scrollTop).toBeLessThan(end());
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  test("a wheel turned up lets go, and growth leaves the reader put", async () => {
    const { box, grow, wheelUp } = setup();
    await grow(0);
    await wheelUp();
    const put = box.scrollTop;
    await grow(200);
    expect(box.scrollTop).toBe(put);
  });

  test("a key that scrolls up lets go at once", async () => {
    const { box, grow } = setup();
    await grow(0);
    act(() => {
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp" }));
    });
    await settle();
    const put = box.scrollTop;
    await grow(200);
    expect(box.scrollTop).toBe(put);
  });

  test("a wheel turned down does not let go", async () => {
    const { box, grow, wheelOn, end } = setup();
    await grow(0);
    await wheelOn(box, 40);
    await grow(200);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// At the very top the wheel moves nothing; a let go there would stop the
  /// following with no scroll able to pick it back up.
  test("a wheel turned up at the very top keeps following", async () => {
    const { box, grow, wheelOn, end } = setup();
    await grow(0);
    box.scrollTop = 0; // no scroll event: the guard is about the wheel itself
    await wheelOn(box);
    await grow(200);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// A preview or a diff scrolls inside its own box; its wheel bubbles up to
  /// the thread without moving it, and must not stop the following.
  test("a wheel over a nested scrollable is its own, not the thread's", async () => {
    const { box, grow, wheelOn, nested, end } = setup();
    await grow(0);
    await wheelOn(nested(200));
    await grow(200);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// At the nested box's own top the wheel chains up to the thread — that
  /// scroll is the thread's, and it lets go.
  test("a wheel over a nested scrollable at its top chains to the thread", async () => {
    const { box, grow, wheelOn, nested } = setup();
    await grow(0);
    const put = box.scrollTop;
    await wheelOn(nested(0));
    await grow(200);
    expect(box.scrollTop).toBe(put);
  });

  /// A box that keeps its overscroll to itself chains nothing past its top:
  /// the wheel there never moves the thread. happy-dom computes no
  /// overscroll-behavior, so the style is posed as the real browser reports it.
  test("a wheel over a nested scrollable with overscroll contain stays its own at its top", async () => {
    const { box, grow, wheelOn, nested, end } = setup();
    await grow(0);
    const inner = nested(0);
    const real = globalThis.getComputedStyle;
    globalThis.getComputedStyle = ((el: Element) => {
      const style = real(el);
      if (el !== inner) return style;
      return new Proxy(style, {
        get: (t, k) => (k === "overscrollBehaviorY" ? "contain" : Reflect.get(t, k)),
      });
    }) as typeof real;
    try {
      await wheelOn(inner);
      await grow(200);
      await glide();
      expect(box.scrollTop).toBeCloseTo(end(), 0);
    } finally {
      globalThis.getComputedStyle = real;
    }
  });

  /// A thread shorter than its box has nowhere to scroll up to: a let go
  /// there would stop the following with no scroll able to pick it back up.
  test("a wheel turned up on a thread shorter than its box keeps following", async () => {
    const { box, grow, wheelUp, end } = setup(200);
    await grow(0);
    await wheelUp();
    await grow(500);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  test("a drag up lets go, and the thread stays where the reader left it", async () => {
    const { box, grow, userScrollsTo } = setup();
    await grow(200);
    await userScrollsTo(400);
    await userScrollsTo(380);
    await grow(200);
    expect(box.scrollTop).toBe(380);
  });

  test("a lone scrollbar click lets go on its single read", async () => {
    const { box, grow, userScrollsTo } = setup();
    await grow(0);
    await userScrollsTo(400);
    await grow(200);
    expect(box.scrollTop).toBe(400);
  });

  test("returning to the end picks following back up", async () => {
    const { box, grow, userScrollsTo, end } = setup();
    await grow(0);
    await userScrollsTo(400);
    await grow(200);
    await userScrollsTo(end());
    await grow(100);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  test("a shrink that clamps the reader onto the end resumes following", async () => {
    const { box, grow, shrink, wheelUp, end } = setup();
    await grow(0);
    await wheelUp();
    await shrink(650); // the end moves above the reader; the box is clamped onto it
    expect(box.scrollTop).toBe(end());
    await grow(100);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  test("scrollToBottom follows again even from far up, instantly", async () => {
    const { box, grow, userScrollsTo, end, api } = setup();
    await grow(0);
    await userScrollsTo(0);
    await act(async () => {
      api().scrollToBottom();
      await Bun.sleep(80);
    });
    expect(box.scrollTop).toBe(end());
    await grow(50);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  test("isAtBottom is false away from the end and true back at it", async () => {
    const { grow, userScrollsTo, end, api } = setup();
    await grow(0);
    expect(api().isAtBottom).toBe(true);
    await userScrollsTo(400);
    expect(api().isAtBottom).toBe(false);
    await userScrollsTo(end());
    expect(api().isAtBottom).toBe(true);
  });

  /// Typing grows the message box, which takes its height from the thread.
  /// That is not the answer growing: the hook observes the content alone, so
  /// the box's own resize is nothing to it.
  test("only the content is observed, never the scrolling box", async () => {
    const { box } = setup();
    expect(resized.map((r) => r.target)).toEqual([box.firstChild]);
  });

  /// The hook's ref returns a cleanup, so React never calls it with null:
  /// the listeners the hook added must be removed by that cleanup, or a
  /// closed chat keeps answering the wheel and the keys. The library's own
  /// scroll/wheel stay on the detached element — its useRefCallback nulls
  /// current before removing, so its unmount removes nothing either; they
  /// ride off with the element's garbage, inert against a null current.
  test("unmount removes the hook's own listeners from the box", () => {
    const ops: { target: EventTarget; type: string; delta: number }[] = [];
    // The method lives on happy-dom's internal EventTarget prototype, which
    // the global EventTarget does not name — find its real owner by the chain.
    let owner: object = document.createElement("div");
    while (!Object.prototype.hasOwnProperty.call(owner, "addEventListener")) {
      owner = Object.getPrototypeOf(owner);
    }
    const proto = owner as { addEventListener: typeof EventTarget.prototype.addEventListener; removeEventListener: typeof EventTarget.prototype.removeEventListener };
    const realAdd = proto.addEventListener;
    const realRemove = proto.removeEventListener;
    proto.addEventListener = function (type: string, ...rest: unknown[]) {
      ops.push({ target: this, type, delta: 1 });
      return (realAdd as (...a: unknown[]) => void).call(this, type, ...rest);
    } as typeof realAdd;
    proto.removeEventListener = function (type: string, ...rest: unknown[]) {
      ops.push({ target: this, type, delta: -1 });
      return (realRemove as (...a: unknown[]) => void).call(this, type, ...rest);
    } as typeof realRemove;
    try {
      const rendered = setup();
      rendered.unmount();
      const balance = new Map<string, number>();
      for (const op of ops) {
        if (op.target !== rendered.box) continue;
        balance.set(op.type, (balance.get(op.type) ?? 0) + op.delta);
      }
      // The hook's wheel and keydown are removed by its cleanup. The
      // library's scroll/wheel physically stay on the detached element (its
      // unmount removes nothing), but its attach removes from the just-set
      // current first, so those phantom removals balance the count to zero.
      expect([...balance.keys()].sort()).toEqual(["keydown", "scroll", "wheel"]);
      expect([...balance.values()].every((n) => n === 0)).toBe(true);
    } finally {
      proto.addEventListener = realAdd;
      proto.removeEventListener = realRemove;
    }
  });
});

describe("the back-to-bottom button", () => {
  test("is hidden at the end and shown away from it", () => {
    const { queryByTitle, rerender } = render(
      <ScrollToBottomButton atBottom={true} onClick={() => {}} />,
    );
    expect(queryByTitle("Back to the end of the conversation")).toBeNull();
    rerender(<ScrollToBottomButton atBottom={false} onClick={() => {}} />);
    expect(queryByTitle("Back to the end of the conversation")).not.toBeNull();
  });

  test("a click asks for the end", () => {
    let asked = 0;
    const { getByTitle } = render(
      <ScrollToBottomButton atBottom={false} onClick={() => (asked += 1)} />,
    );
    fireEvent.click(getByTitle("Back to the end of the conversation"));
    expect(asked).toBe(1);
  });
});