import { describe, expect, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import { usePages } from "../hooks/usePages";
import { pagesShown } from "../components/pages";

const shown = (on: string[] = []) => pagesShown((f) => on.includes(f));
const ids = (pages: readonly { id: string }[]) => pages.map((one) => one.id);

describe("the page in the chat's place", () => {
  test("opens on what another page sent; the sidebar's open carries none", () => {
    const { result } = renderHook(() => usePages(false, shown(["meeting-notes"])));
    act(() => result.current.openPage("meetings", "m1"));
    expect(result.current.onPage?.id).toBe("meetings");
    expect(result.current.pageItem).toBe("m1");
    act(() => result.current.openPage("catalog"));
    expect(result.current.onPage?.id).toBe("catalog");
    expect(result.current.pageItem).toBeUndefined();
  });

  test("leaving keeps what it was opened on for the next open", () => {
    const { result } = renderHook(() => usePages(false, shown(["meeting-notes"])));
    act(() => result.current.openPage("meetings", "m1"));
    act(() => result.current.leavePage());
    expect(result.current.onPage).toBeNull();
    expect(result.current.pageItem).toBe("m1");
  });

  test("a keep page stays mounted after leaving; an ordinary one does not", () => {
    const { result } = renderHook(() => usePages(false, shown(["mongodb"])));
    act(() => result.current.openPage("mongodb"));
    act(() => result.current.openPage("catalog"));
    expect(ids(result.current.keptPages)).toEqual(["mongodb"]);
    act(() => result.current.leavePage());
    expect(ids(result.current.keptPages)).toEqual(["mongodb"]);
  });

  test("a feature turned off drops its kept page; turned on again it waits to be opened", () => {
    const { result, rerender } = renderHook(({ on }) => usePages(false, shown(on)), {
      initialProps: { on: ["mongodb"] },
    });
    act(() => result.current.openPage("mongodb"));
    act(() => result.current.leavePage());
    expect(ids(result.current.keptPages)).toEqual(["mongodb"]);
    rerender({ on: [] });
    expect(result.current.keptPages).toEqual([]);
    rerender({ on: ["mongodb"] });
    expect(result.current.keptPages).toEqual([]);
    act(() => result.current.openPage("mongodb"));
    expect(ids(result.current.keptPages)).toEqual(["mongodb"]);
  });

  test("a feature's page turned off while open closes", () => {
    const { result, rerender } = renderHook(({ on }) => usePages(false, shown(on)), {
      initialProps: { on: ["board"] },
    });
    act(() => result.current.openPage("board"));
    expect(result.current.onPage?.id).toBe("board");
    rerender({ on: [] });
    expect(result.current.onPage).toBeNull();
  });

  test("the IDE layout shows no page, and it comes back when the layout leaves", () => {
    const { result, rerender } = renderHook(({ ide }) => usePages(ide, shown()), {
      initialProps: { ide: false },
    });
    act(() => result.current.openPage("catalog"));
    rerender({ ide: true });
    expect(result.current.onPage).toBeNull();
    rerender({ ide: false });
    expect(result.current.onPage?.id).toBe("catalog");
  });

  test("a keep page opened in the IDE layout is not mounted there", () => {
    const { result } = renderHook(() => usePages(true, shown(["mongodb"])));
    act(() => result.current.openPage("mongodb"));
    expect(result.current.onPage).toBeNull();
    expect(result.current.keptPages).toEqual([]);
  });
});
