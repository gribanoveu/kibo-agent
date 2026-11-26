import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { useFollowBottom } from "../hooks/useFollowBottom";

// The chat thread stays at its end while the answer grows, and lets go of it
// the moment the user scrolls up to read. The hook knows every position it
// wrote itself and cancels those reads; a read at a position it never wrote is
// the user's, and lets go at once — no second read, no pause in the sticking.

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
  // A tool card swapping its preview for a result: the thread shorter.
  const shrink = (by: number) => {
    height -= by;
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
  return { box, grow, shrink, resizeBox, userScrollsTo, api: () => api };
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

  /// A drag sends a run of reads at positions the hook never wrote: the first
  /// is the user, and the thread stays where they left it.
  test("a drag up lets go on its first read, and the thread stays put", () => {
    const { box, grow, userScrollsTo } = setup();
    grow(200);
    userScrollsTo(400);
    userScrollsTo(380);
    grow(200);
    expect(box.scrollTop).toBe(380);
  });

  /// A lone scrollbar-track click sends one read at a position we never wrote.
  /// It is the user as much as a drag is: no second read to confirm it.
  test("a lone scrollbar click lets go on its single read", () => {
    const { box, grow, userScrollsTo } = setup();
    grow(0);
    userScrollsTo(400);
    grow(200);
    expect(box.scrollTop).toBe(400);
  });

  test("returning to the end picks following back up", () => {
    const { box, grow, userScrollsTo } = setup();
    grow(0);
    userScrollsTo(400);
    grow(200);
    userScrollsTo(900); // 1200 - 300: the end again, a position we never wrote
    grow(100);
    expect(box.scrollTop).toBe(1300);
  });

  /// The end may be the very position a stick left, with the answer at rest:
  /// past the echo window that read is the user's return, not our echo.
  test("a return to the same end after the echo window follows again", async () => {
    const { box, grow, userScrollsTo } = setup();
    grow(0);
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    await Bun.sleep(200);
    userScrollsTo(1000);
    grow(100);
    expect(box.scrollTop).toBe(1100);
  });

  /// Under load WebKit's scroll event reads a commit older than the last
  /// stick — a position we wrote a frame ago. It is ours, so it neither lets
  /// go nor pauses the stick: the next word still takes the end at once.
  test("a stale commit of our own write neither lets go nor pauses the stick", () => {
    const { box, grow } = setup();
    grow(200); // the stick leaves the box at 1200
    grow(400); // and at 1600
    box.scrollTop = 1200; // the event reads the older commit
    box.dispatchEvent(new Event("scroll"));
    grow(400);
    expect(box.scrollTop).toBe(2000);
    grow(100);
    expect(box.scrollTop).toBe(2100);
  });

  /// A tool card swapping its preview for a result shortens the thread and
  /// clamps the box onto the end the stick just wrote. happy-dom does not
  /// clamp scrollTop, so the clamp's read is played as a read of that logged
  /// position once growth has made it away from the end — the `ours` path; in
  /// a real DOM the same read is at the end as well.
  test("a read of the position the shrink's stick wrote is ours, not the user", () => {
    const { box, grow, shrink } = setup();
    grow(600);
    shrink(600); // the stick takes the shorter end, 1000
    grow(800); // the end moves on to 1800; 1000 is now away from it
    box.scrollTop = 1000; // the clamp's read, a position we wrote
    box.dispatchEvent(new Event("scroll"));
    grow(100);
    expect(box.scrollTop).toBe(1900);
  });

  /// The answer at rest: the last stick is older than the echo window when the
  /// user lets go, and the commit's echo still reads the end. The window runs
  /// from the let go, not from the write, or the echo re-follows and the next
  /// word yanks the reader back down.
  test("an end echo after a let go over an old stick does not re-follow", async () => {
    const { box, grow } = setup();
    grow(0);
    await Bun.sleep(200); // the stick is now older than the echo window
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    box.dispatchEvent(new Event("scroll")); // the echo reads the end
    grow(200);
    expect(box.scrollTop).toBe(1000);
    // The user's own return to the end, past the window: following again.
    await Bun.sleep(200);
    box.scrollTop = 1200;
    box.dispatchEvent(new Event("scroll"));
    grow(100);
    expect(box.scrollTop).toBe(1300);
  });

  /// A shrink while the user reads above can clamp their position onto the new
  /// end: they are at the end again, and following resumes. A pinned choice —
  /// the reader was in the tail the shrink removed — not an accident.
  test("a clamp to the end while reading resumes following", async () => {
    const { box, grow, shrink, userScrollsTo } = setup();
    grow(200);
    box.dispatchEvent(new WheelEvent("wheel", { deltaY: -40 }));
    await Bun.sleep(200); // past the echo window
    userScrollsTo(400);
    shrink(700); // height 500: the end is 200, and the clamp puts the box there
    box.scrollTop = 200;
    box.dispatchEvent(new Event("scroll"));
    grow(100);
    expect(box.scrollTop).toBe(600);
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
  /// never be picked back up.
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
