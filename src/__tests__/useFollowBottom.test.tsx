import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, fireEvent, render } from "@testing-library/react";
import { FollowThread } from "../components/FollowThread";

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
  painted = 0;
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

/** The panel as the app composes it: the thread owns the following hook and
 *  the button, and `painted` counts how often the panel around it renders —
 *  a scroll must not repaint the conversation. */
let painted = 0;
function Page({ open = "chat", sent = "0" }: { open?: string; sent?: string }) {
  painted += 1;
  return (
    <FollowThread className="thread" open={open} sent={sent}>
      <div data-testid="content" />
    </FollowThread>
  );
}

/** Let the library's frames and its 1ms scroll reads run. */
const settle = (ms = 80) => act(async () => void (await Bun.sleep(ms)));
/** Let the spring catch up: from far up it approaches the end over ~1.5s. */
const glide = () => settle(1600);

function setup(start = 1000) {
  const view = render(<Page />);
  const box = view.container.querySelector(".thread") as HTMLElement;
  const content = view.container.querySelector(".thread-content") as HTMLElement;
  // As the real thread: overflow-y auto with overflow-x hidden, so the computed
  // overflow shorthand is not a bare "auto" — the library's own wheel path
  // walks past it, and the hook's own listener must let go.
  box.style.overflowY = "auto";
  box.style.overflowX = "hidden";
  let height = start;
  let shown = 300;
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
  // The box itself gets taller — the composer shrank after a send, the window
  // grew: the browser clamps scrollTop back with the content unchanged.
  const boxGrowsTo = async (next: number) => {
    shown = next;
    box.scrollTop = Math.max(0, height - shown);
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
  // Another chat is opened: the panel hands the thread a new `open`.
  const openChat = async (name: string, ms = 80) => {
    view.rerender(<Page open={name} />);
    await settle(ms);
  };
  return {
    box,
    content,
    grow,
    shrink,
    userScrollsTo,
    boxGrowsTo,
    wheelUp,
    wheelOn,
    nested,
    end,
    openChat,
    queryByTitle: view.queryByTitle,
    getByTitle: view.getByTitle,
    unmount: view.unmount,
    painted: () => painted,
  };
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

  /// The composer shrank after a send: the box got taller, and the browser
  /// clamped `scrollTop` back with the content unchanged. Not the reader
  /// scrolling up — the library observes the content alone and would read the
  /// step as one, stopping the following with the thread still at its end.
  test("a box grown under the reader does not let go of the end", async () => {
    const { box, grow, boxGrowsTo, end } = setup();
    await grow(200);
    await settle();
    await boxGrowsTo(400); // +100px of box, the clamp pulls scrollTop back
    await grow(300);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// WebKit hands the position over late: a read can report a position the
  /// spring left a frame or two ago. One of ours, not the reader's — the
  /// library would take the step back for scrolling up and let go.
  test("a commit of our own write a frame late does not let go", async () => {
    const { box, grow, end } = setup();
    await grow(200);
    await settle();
    await grow(400); // the spring is on its way to the new end
    const written = box.scrollTop;
    await settle(20); // a frame or two more of writes
    box.scrollTop = written; // the event reads the earlier commit
    act(() => {
      box.dispatchEvent(new Event("scroll"));
    });
    await settle();
    await grow(400);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// The spring writes every frame and the browser reports each write, so a
  /// read of ours is the frame before, not the whole stream behind it. A drag
  /// that lands where the spring passed must still let go.
  test("a drag inside the ground the spring covered lets go", async () => {
    const { box, grow, userScrollsTo } = setup();
    for (let i = 0; i < 20; i += 1) {
      await grow(30); // a word of the answer, over and over
      // The browser reports each write of the spring; this read stands in.
      act(() => {
        box.dispatchEvent(new Event("scroll"));
      });
      await settle(10);
    }
    const put = box.scrollTop;
    await userScrollsTo(put - 100); // a throw up through it
    await grow(30);
    expect(box.scrollTop).toBe(put - 100);
  });

  /// Under load the commits come late one after another: the first is ours, and
  /// the next reports the write that followed it — a trail thrown away with
  /// every read would lose it and let go.
  test("a second late commit in a row does not let go", async () => {
    const { box, grow, end } = setup();
    await grow(200);
    await settle();
    await grow(600); // the spring is on its way
    const first = box.scrollTop;
    await settle(20);
    const second = box.scrollTop;
    await settle(20);
    for (const late of [first, second]) {
      box.scrollTop = late;
      act(() => {
        box.dispatchEvent(new Event("scroll"));
      });
      await settle();
    }
    await grow(400);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// Opening a chat is not a read through it: the thread lands at its end at
  /// once, where a glide through a long history reads as a lag.
  test("an opened chat lands at the end at once", async () => {
    const { box, grow, userScrollsTo, end, openChat } = setup();
    await grow(0);
    await userScrollsTo(0);
    await openChat("another");
    expect(box.scrollTop).toBe(end());
  });

  /// Typing grows the message box, which takes its height from the thread.
  /// That is not the answer growing: the hook observes the content alone, so
  /// the box's own resize is nothing to it.
  test("only the content is observed, never the scrolling box", async () => {
    const { box, content } = setup();
    expect(resized.length).toBeGreaterThan(0);
    expect(resized.every((r) => r.target === content && r.target !== box)).toBe(true);
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
  test("is hidden at the end and shown away from it", async () => {
    const { grow, userScrollsTo, end, queryByTitle } = setup();
    await grow(0);
    expect(queryByTitle("Back to the end of the conversation")).toBeNull();
    await userScrollsTo(400);
    expect(queryByTitle("Back to the end of the conversation")).not.toBeNull();
    await userScrollsTo(end());
    expect(queryByTitle("Back to the end of the conversation")).toBeNull();
  });

  test("a click glides back to the end, and following carries on", async () => {
    const { box, grow, userScrollsTo, end, queryByTitle, getByTitle } = setup();
    await grow(0);
    await userScrollsTo(400);
    fireEvent.click(getByTitle("Back to the end of the conversation"));
    // The reader has asked for the end: the button goes at once, not when the
    // spring arrives.
    expect(queryByTitle("Back to the end of the conversation")).toBeNull();
    // Mid-glide: the spring is on its way, not snapped to the end.
    await settle();
    expect(box.scrollTop).toBeGreaterThan(0);
    expect(box.scrollTop).toBeLessThan(end());
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
    expect(queryByTitle("Back to the end of the conversation")).toBeNull();
    await grow(100);
    await glide();
    expect(box.scrollTop).toBeCloseTo(end(), 0);
  });

  /// The button subscribes to the thread's end itself: the panel around it is
  /// the whole conversation, and showing or hiding it must not repaint that.
  test("showing and hiding it does not repaint the conversation", async () => {
    const { grow, userScrollsTo, end, painted: repaints } = setup();
    await grow(0);
    const before = repaints();
    await userScrollsTo(400);
    await userScrollsTo(end());
    expect(repaints()).toBe(before);
  });
});