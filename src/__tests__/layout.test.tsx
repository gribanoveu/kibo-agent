import { afterEach, describe, expect, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { isBoolean, useStoredState } from "../hooks/useStoredState";
import { PANEL_LIMITS, roomFor, usePanelSizes } from "../hooks/usePanelSizes";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { PanelResizeHandle } from "../components/PanelResizeHandle";
import { Columns } from "../components/Columns";
import { isAsideTab } from "../types";

afterEach(() => localStorage.clear());

describe("a stored layout", () => {
  test("comes back on the next start", () => {
    const first = renderHook(() => useStoredState("k", false, isBoolean));
    act(() => first.result.current[1](true));
    first.unmount();
    const next = renderHook(() => useStoredState("k", false, isBoolean));
    expect(next.result.current[0]).toBe(true);
  });

  test("nothing stored, damaged, or of another shape is the default", () => {
    for (const raw of [null, "{oops", '"yes"']) {
      if (raw !== null) localStorage.setItem("k", raw);
      const { result, unmount } = renderHook(() => useStoredState("k", false, isBoolean));
      expect(result.current[0]).toBe(false);
      unmount();
    }
  });

  test("a tab since renamed opens the default one", () => {
    localStorage.setItem("tab", '"gone"');
    const { result } = renderHook(() => useStoredState("tab", "changes", isAsideTab));
    expect(result.current[0]).toBe("changes");
    expect(isAsideTab("terminal")).toBe(true);
  });
});

describe("panel widths", () => {
  const controls = {
    sidebar: { collapsed: false, collapse: () => {}, expand: () => {} },
    aside: { collapsed: false, collapse: () => {}, expand: () => {} },
  };

  test("are kept across starts", () => {
    const first = renderHook(() => usePanelSizes(controls));
    act(() => first.result.current.resizeSidebarBy(40));
    first.unmount();
    const next = renderHook(() => usePanelSizes(controls));
    expect(next.result.current.widths.sidebar).toBe(PANEL_LIMITS.sidebar.initial + 40);
  });

  /// The side panel is hidden from the chat header, not by dragging: pushed
  /// past its minimum it stops there. The sidebar still snaps to its rail.
  test("the side panel stops at its minimum; the sidebar collapses", () => {
    let collapsed = 0;
    const { result } = renderHook(() =>
      usePanelSizes({ sidebar: { collapsed: false, collapse: () => collapsed++, expand: () => {} } }),
    );
    act(() => result.current.resizeAsideBy(-2000));
    expect(result.current.widths.aside).toBe(PANEL_LIMITS.aside.min);

    act(() => result.current.resizeSidebarBy(-2000));
    expect(result.current.widths.sidebar).toBe(PANEL_LIMITS.sidebar.min);
    expect(collapsed).toBe(1);
  });

  test("the bottom panel stops at its minimum height", () => {
    const { result } = renderHook(() => usePanelSizes({}));
    act(() => result.current.resizeBottomBy(60));
    expect(result.current.widths.bottom).toBe(PANEL_LIMITS.bottom.initial + 60);
    act(() => result.current.resizeBottomBy(-2000));
    expect(result.current.widths.bottom).toBe(PANEL_LIMITS.bottom.min);
    act(() => result.current.resizeBottomBy(30));
    expect(result.current.widths.bottom).toBe(PANEL_LIMITS.bottom.min + 30);
  });

  test("the file viewer is sized too, and widths stored before it existed are kept", () => {
    localStorage.setItem("atlas-panel-widths", JSON.stringify({ sidebar: 300, aside: 400, bottom: 200 }));
    const { result } = renderHook(() => usePanelSizes({}));
    expect(result.current.widths).toEqual({
      sidebar: 300,
      aside: 400,
      bottom: 200,
      viewer: PANEL_LIMITS.viewer.initial,
      chat: PANEL_LIMITS.chat.initial,
    });
    act(() => result.current.resizeViewerBy(100));
    expect(result.current.widths.viewer).toBe(PANEL_LIMITS.viewer.initial + 100);
    act(() => result.current.resizeViewerBy(-5000));
    expect(result.current.widths.viewer).toBe(PANEL_LIMITS.viewer.min);
    expect(result.current.widths.aside).toBe(400);
  });

  /// Dragged wider than the window has room for, the panel stops short; its
  /// width comes down to where it stopped, or the next drag back would first
  /// take back width that never showed.
  test("a drag that ran out of room leaves the width where the panel stopped", () => {
    const { result } = renderHook(() => usePanelSizes({}));
    act(() => result.current.resizeAsideBy(200));
    expect(result.current.widths.aside).toBe(PANEL_LIMITS.aside.initial + 200);
    act(() => result.current.endResize("aside", 331.6));
    expect(result.current.widths.aside).toBe(332);
    // A panel drawn as wide as asked, or wider, keeps what was asked.
    act(() => result.current.endResize("aside", 900));
    expect(result.current.widths.aside).toBe(332);
    act(() => result.current.endResize());
    expect(result.current.widths.aside).toBe(332);
  });

  test("a collapsed panel's width is not brought down to its rail", () => {
    const { result } = renderHook(() =>
      usePanelSizes({ sidebar: { collapsed: true, collapse: () => {}, expand: () => {} } }),
    );
    act(() => result.current.endResize("sidebar", 58));
    expect(result.current.widths.sidebar).toBe(PANEL_LIMITS.sidebar.initial);
  });

  test("outside today's limits are not trusted", () => {
    localStorage.setItem("atlas-panel-widths", JSON.stringify({ sidebar: 9999, aside: 300 }));
    const { result } = renderHook(() => usePanelSizes(controls));
    expect(result.current.widths.sidebar).toBe(PANEL_LIMITS.sidebar.initial);
  });
});

describe("room for the panels", () => {
  /// What the desktop window's widths (tauri.conf.json) are set from: at its
  /// minimum the chat and the file viewer fit beside the rail, and the
  /// sidebar, the chat and the side panel side by side; at its default, all.
  test("the smallest window holds the viewer beside the rail, or the side panel beside the sidebar", () => {
    expect(roomFor({ rail: true, viewer: true, dock: false, frame: 2 })).toBe(940);
    expect(roomFor({ rail: false, viewer: false, dock: true, frame: 2 })).toBe(882);
    expect(roomFor({ rail: false, viewer: false, dock: false, frame: 2 })).toBe(612);
  });

  test("the default window holds every panel; narrower, the sidebar's rail or the side panel gives way", () => {
    expect(roomFor({ rail: false, viewer: true, dock: true, frame: 2 })).toBe(1332);
    expect(roomFor({ rail: true, viewer: true, dock: true, frame: 2 })).toBe(1210);
    expect(roomFor({ rail: false, viewer: true, dock: false, frame: 0 })).toBe(1060);
  });
});

describe("a media query", () => {
  const original = window.matchMedia;
  afterEach(() => {
    window.matchMedia = original;
  });

  test("answers now, and again when the window crosses it", () => {
    let matches = false;
    let changed = () => {};
    window.matchMedia = (() => ({
      get matches() {
        return matches;
      },
      addEventListener: (_: string, fn: () => void) => (changed = fn),
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
    const { result } = renderHook(() => useMediaQuery("(min-width: 1px)"));
    expect(result.current).toBe(false);
    matches = true;
    act(() => changed());
    expect(result.current).toBe(true);
  });
});

describe("the resize handle", () => {
  /// It sizes the panel beside it — after it when `invert` — and says how
  /// large that panel came out once the drag ends.
  test("ends a drag with the size of the panel it sizes", () => {
    const ends: (number | undefined)[] = [];
    const panel = (width: number) => (el: HTMLDivElement | null) => {
      if (el) el.getBoundingClientRect = () => ({ width, height: width / 2 }) as DOMRect;
    };
    render(
      <div>
        <div ref={panel(111)} />
        <PanelResizeHandle ariaLabel="left" onResize={() => {}} onResizeEnd={(size) => ends.push(size)} />
        <div ref={panel(222)} />
        <PanelResizeHandle ariaLabel="right" invert onResize={() => {}} onResizeEnd={(size) => ends.push(size)} />
        <div ref={panel(333)} />
        <PanelResizeHandle ariaLabel="down" axis="y" invert onResize={() => {}} onResizeEnd={(size) => ends.push(size)} />
        <div ref={panel(444)} />
      </div>,
    );
    for (const name of ["left", "right", "down"]) {
      fireEvent.pointerDown(screen.getByRole("separator", { name }), { clientX: 0, clientY: 0 });
      act(() => {
        window.dispatchEvent(new Event("pointerup"));
      });
    }
    expect(ends).toEqual([111, 333, 222]);
  });
});

describe("the columns", () => {
  /// Keeps what was typed: switching layouts must move the chat, not draw it anew.
  function Draft() {
    return <input aria-label="draft" />;
  }
  const columns = (ide: boolean, viewer: boolean, bottom = false) => {
    const { result } = renderHook(() => usePanelSizes({}));
    return (
      <div data-testid="body">
        <Columns
          ide={ide}
          chat={<main key="chat" className="main"><Draft /></main>}
          viewer={viewer ? <section key="viewer" className="file-viewer" /> : null}
          top={<aside className="aside-right" />}
          topShown
          bottom={bottom ? <aside className="aside-bottom" /> : null}
          panels={result.current}
        />
      </div>
    );
  };
  const names = (parent: Element) => [...parent.children].map((el) => el.getAttribute("aria-label") ?? el.className);
  const order = () => names(screen.getByTestId("body"));

  test("chat first, then the file and the panes; the IDE layout turns it around", () => {
    const { rerender } = render(columns(false, true, true));
    expect(order()).toEqual(["main", "Resize the file viewer", "file-viewer", "Resize the side panel", "dock-column"]);
    expect(names(document.querySelector(".dock-column")!)).toEqual(["aside-right", "Resize the bottom panel", "aside-bottom"]);
    fireEvent.change(screen.getByLabelText("draft"), { target: { value: "half a sentence" } });

    rerender(columns(true, true, true));
    // With a file, the middle's minimum is the viewer's, not every open tab's width.
    expect(order()).toEqual(["dock-column", "Resize the side panel", "ide-middle has-file", "Resize the chat", "main"]);
    expect((screen.getByLabelText("draft") as HTMLInputElement).value).toBe("half a sentence");
    // The tree alone on the left; the bottom dock under the file, as an editor's terminal.
    expect(names(document.querySelector(".dock-column")!)).toEqual(["aside-right"]);
    expect(names(document.querySelector(".ide-middle")!)).toEqual(["file-viewer", "Resize the bottom panel", "aside-bottom"]);

    // No file open: the middle still holds the chat on the right.
    rerender(columns(true, false));
    expect(names(document.querySelector(".ide-middle")!)).toEqual(["ide-empty"]);
    expect(document.querySelector(".ide-middle")!.className).toBe("ide-middle");
  });
});
