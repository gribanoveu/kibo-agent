import { describe, expect, test } from "bun:test";
import { renderHook } from "@testing-library/react";
import { useFolderConversation } from "../hooks/useFolderConversation";

type Props = { path: string | null; resumed: boolean; latest?: string; blocks?: unknown[] };

function setup(initial: Props) {
  const opened: string[] = [];
  let resets = 0;
  const agent = (blocks: unknown[] = []) => ({
    turn: { blocks },
    open: (id: string) => opened.push(id),
    reset: () => resets++,
  });
  const hook = renderHook((p: Props) => useFolderConversation(p.path, p.resumed, p.latest, agent(p.blocks)), {
    initialProps: initial,
  });
  return { ...hook, opened, resets: () => resets };
}

describe("coming back to a folder", () => {
  test("its latest conversation opens once the list arrives", () => {
    const h = setup({ path: "/a", resumed: true });
    expect(h.opened).toEqual([]);
    h.rerender({ path: "/a", resumed: true, latest: "c2" });
    expect(h.opened).toEqual(["c2"]);
  });

  test("once: a later change of the list opens nothing", () => {
    const h = setup({ path: "/a", resumed: true, latest: "c2" });
    h.rerender({ path: "/a", resumed: true, latest: "c3" });
    expect(h.opened).toEqual(["c2"]);
  });

  test("not over something already on screen", () => {
    const h = setup({ path: "/a", resumed: true, latest: "c2", blocks: [{}] });
    expect(h.opened).toEqual([]);
  });

  // F-8.6: the comeback is the launch's. A folder with no chat to reopen
  // leaves it armed, and a folder opened later must not take it.
  test("not after the user has moved to another folder", () => {
    const h = setup({ path: "/a", resumed: true });
    h.rerender({ path: "/b", resumed: true });
    h.rerender({ path: "/b", resumed: true, latest: "b1" });
    expect(h.opened).toEqual([]);
  });

  test("not for a folder chosen by hand", () => {
    const h = setup({ path: "/a", resumed: false, latest: "c2" });
    expect(h.opened).toEqual([]);
  });
});

describe("switching folders", () => {
  test("starts a fresh thread", () => {
    const h = setup({ path: "/a", resumed: false });
    h.rerender({ path: "/b", resumed: false });
    expect(h.resets()).toBe(1);
    h.rerender({ path: "/b", resumed: false });
    expect(h.resets()).toBe(1);
  });

  test("but the first folder opened is not a switch, nor is a re-render", () => {
    const h = setup({ path: null, resumed: false });
    h.rerender({ path: "/a", resumed: true });
    h.rerender({ path: "/a", resumed: true });
    expect(h.resets()).toBe(0);
  });
});
