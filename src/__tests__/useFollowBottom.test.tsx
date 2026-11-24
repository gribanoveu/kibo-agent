import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { useFollowBottom } from "../hooks/useFollowBottom";

// The chat thread stays at its end while the answer grows, and lets go of it
// the moment the user scrolls up to read.

// One entry per observed node, so a test can grow the content and resize the
// box apart — they mean different things to the hook.
let resized: { target: Element; cb: () => void }[] = [];
const RealObserver = globalThis.ResizeObserver;
beforeEach(() => {
  resized = [];
  globalThis.ResizeObserver = class {
    constructor(private cb: () => void) {}
    observe(target: Element) {
      resized.push({ target, cb: this.cb });
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
    <div ref={api.scrollRef} data-testid="box">
      <div ref={api.contentRef} data-testid="content" />
    </div>
  );
}

function setup(start = 1000) {
  let api!: ReturnType<typeof useFollowBottom>;
  const { getByTestId } = render(<Thread onReady={(a) => (api = a)} />);
  const box = getByTestId("box");
  const content = getByTestId("content");
  const fire = (target: Element) => {
    for (const { target: node, cb } of resized) if (node === target) cb();
  };
  let height = start;
  let shown = 300;
  Object.defineProperty(box, "scrollHeight", { get: () => height });
  Object.defineProperty(box, "clientHeight", { get: () => shown });
  // The answer grows a word longer.
  const grow = (by: number) => {
    height += by;
    fire(content);
  };
  // The message box takes its height from the thread: a line typed, a line sent.
  const resizeBox = (by: number) => {
    shown += by;
    fire(box);
  };
  const userScrollsTo = (top: number) => {
    box.scrollTop = top;
    box.dispatchEvent(new Event("scroll"));
  };
  return { box, grow, resizeBox, userScrollsTo, api: () => api };
}

describe("following the end of the thread", () => {
  test("scrolls to the end as the content grows", () => {
    const { box, grow } = setup();
    grow(200);
    expect(box.scrollTop).toBe(1200);
  });

  /// Typing grows the message box, which takes its height from the thread. That
  /// is not the answer growing: what is being read must not move by a line for
  /// every line typed, and the end waits for the next word of the answer.
  test("a taller message box leaves the thread where it is", () => {
    const { box, grow, resizeBox } = setup();
    grow(0);
    box.scrollTop = 700; // the end: 1000 - 300
    resizeBox(-42); // two lines typed
    expect(box.scrollTop).toBe(700);
    resizeBox(42); // sent, and the box back to one line
    expect(box.scrollTop).toBe(700);
    grow(200); // the answer's next word takes the end again
    expect(box.scrollTop).toBe(1200);
  });

  test("lets go once the user scrolls up, and picks up again at the end", () => {
    const { box, grow, userScrollsTo } = setup();
    grow(0);
    // A drag sends a run of scroll events; the second confirms the first.
    userScrollsTo(400);
    userScrollsTo(380);
    grow(200);
    expect(box.scrollTop).toBe(380);

    userScrollsTo(900); // 1200 - 300: the end again
    grow(100);
    expect(box.scrollTop).toBe(1300);
  });

  /// Under load WebKit's scroll event reads a commit older than the last
  /// stick — the box is at the end, the event says otherwise. Taking that for
  /// the user stopped the thread following the answer mid-stream.
  test("a stale commit read while the answer streams does not let go", () => {
    const { box, grow } = setup();
    grow(600); // the stick leaves the box at 1600
    box.scrollTop = 1200; // the event reads an older commit
    box.dispatchEvent(new Event("scroll"));
    grow(400); // the stick holds while the read is unconfirmed
    // The next read is back above the suspect: the commit caught up.
    box.scrollTop = 1350;
    box.dispatchEvent(new Event("scroll"));
    expect(box.scrollTop).toBe(2000);
    grow(100);
    expect(box.scrollTop).toBe(2100);
  });

  test("an unconfirmed read re-sticks after a moment", async () => {
    const { box, grow } = setup();
    grow(600);
    box.scrollTop = 1200;
    box.dispatchEvent(new Event("scroll"));
    grow(100);
    expect(box.scrollTop).toBe(1200); // held, not yet taken for the user
    await Bun.sleep(200);
    expect(box.scrollTop).toBe(1700); // nothing confirmed it: back to the end
    grow(100);
    expect(box.scrollTop).toBe(1800);
  });

  /// A wheel turned up where the box cannot move (the top, or a thread still
  /// shorter than the box) is not a scroll away: letting go there would stop
  /// the following with no scroll event ever able to pick it back up.
  test("a wheel turned up at the top keeps following", () => {
    const { box, grow } = setup();
    grow(0);
    box.scrollTop = 0;
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    grow(200);
    expect(box.scrollTop).toBe(1200);
  });

  /// An overflow of a pixel or two leaves the box at the end within the
  /// slack: a wheel up there moves nothing worth reading, and a let go would
  /// never be picked back up — every read stays "at the end" while stuckAt is
  /// null, so the end-check never re-follows.
  test("a wheel turned up with a pixel of overflow keeps following", () => {
    const { box, grow } = setup(301);
    grow(0);
    box.scrollTop = 1; // the most a pixel of overflow allows
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    grow(500);
    expect(box.scrollTop).toBe(801);
  });

  test("a key that scrolls up lets go at once", () => {
    const { box, grow } = setup();
    grow(0);
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp" }));
    grow(200);
    expect(box.scrollTop).toBe(1000);
  });

  /// After a wheel let go the commit still reads the end for a frame; that
  /// echo must not pick the thread back up before the user's scroll arrives.
  test("an end echo right after a wheel let go does not re-follow", () => {
    const { box, grow } = setup();
    grow(0);
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    box.dispatchEvent(new Event("scroll")); // still reads where the stick left it
    grow(200);
    expect(box.scrollTop).toBe(1000);
    // The user's scroll arrives, then they return to the end: following again.
    box.scrollTop = 800;
    box.dispatchEvent(new Event("scroll"));
    box.scrollTop = 1200;
    box.dispatchEvent(new Event("scroll"));
    grow(100);
    expect(box.scrollTop).toBe(1300);
  });

  test("a wheel turned up lets go before the scroll event arrives", () => {
    const { box, grow } = setup();
    grow(0);
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    // WebKit's scroll event still reads where the stick left the box.
    box.dispatchEvent(new Event("scroll"));
    grow(200);
    expect(box.scrollTop).toBe(1000);
  });

  test("a wheel turned down does not let go", () => {
    const { box, grow } = setup();
    grow(0);
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: 40 }));
    grow(200);
    expect(box.scrollTop).toBe(1200);
  });

  test("scrollToBottom follows again even from far up", () => {
    const { box, grow, userScrollsTo, api } = setup();
    grow(0);
    userScrollsTo(0);
    api().scrollToBottom();
    expect(box.scrollTop).toBe(1000);
    grow(50);
    expect(box.scrollTop).toBe(1050);
  });
});
