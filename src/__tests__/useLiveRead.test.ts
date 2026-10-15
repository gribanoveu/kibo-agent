import { describe, expect, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import { useLiveRead } from "../hooks/useLiveRead";

// The one shape behind the board, the agents, the processes and the features:
// read when it starts, read again on the backend's signal, stop when off.

function backend() {
  let list = ["a"];
  let reads = 0;
  const listeners = new Set<() => void>();
  return {
    read: async () => {
      reads++;
      return [...list];
    },
    listen: async (changed: () => void) => {
      listeners.add(changed);
      return () => listeners.delete(changed);
    },
    change: (next: string[]) => {
      list = next;
      listeners.forEach((changed) => changed());
    },
    reads: () => reads,
    listening: () => listeners.size,
  };
}

const settle = () => act(async () => {});

describe("a live read", () => {
  test("reads when it starts and again on the backend's signal", async () => {
    const b = backend();
    const { result } = renderHook(() => useLiveRead(b.read, b.listen, [] as string[]));
    await settle();
    expect(result.current.value).toEqual(["a"]);

    await act(async () => b.change(["a", "b"]));
    expect(result.current.value).toEqual(["a", "b"]);
    expect(b.reads()).toBe(2);
  });

  test("off, it neither reads nor listens; turned off, it stops listening", async () => {
    const b = backend();
    const { rerender } = renderHook(({ on }) => useLiveRead(b.read, b.listen, [] as string[], on), {
      initialProps: { on: false },
    });
    await settle();
    expect(b.reads()).toBe(0);
    expect(b.listening()).toBe(0);

    rerender({ on: true });
    await settle();
    expect(b.listening()).toBe(1);
    rerender({ on: false });
    await settle();
    expect(b.listening()).toBe(0);
  });

  test("a read that fails says why, and the next one clears it", async () => {
    let fail = true;
    const listeners: (() => void)[] = [];
    const { result } = renderHook(() =>
      useLiveRead(
        async () => {
          if (fail) throw "backend down";
          return 1;
        },
        async (changed) => (listeners.push(changed), () => {}),
        0,
      ),
    );
    await settle();
    expect(result.current.error).toBe("backend down");
    fail = false;
    await act(async () => listeners.forEach((changed) => changed()));
    expect(result.current).toMatchObject({ value: 1, error: null });
  });
});
