import { afterEach, describe, expect, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import { useTheme } from "../hooks/useTheme";

// A theme is a mode — light, dark, or as the system is set — and a palette for
// each side. What is written to <html> is the palette on show and its side:
// code colours come only in light and dark, and follow the side.

const root = document.documentElement;
afterEach(() => localStorage.clear());

describe("the theme", () => {
  test("shows the palette of the side the mode picks", () => {
    const { result } = renderHook(() => useTheme());
    act(() => result.current.setMode("light"));
    act(() => result.current.setPalette("chalk"));
    expect([root.dataset.theme, root.dataset.scheme]).toEqual(["chalk", "light"]);
    act(() => result.current.setMode("dark"));
    expect([root.dataset.theme, root.dataset.scheme]).toEqual(["ember", "dark"]);
    // Kept for when light is back.
    expect(result.current.choice.light).toBe("chalk");
  });

  test("a palette of the other side switches to it, unless the system decides", () => {
    const { result } = renderHook(() => useTheme());
    act(() => result.current.setMode("light"));
    act(() => result.current.setPalette("basalt"));
    expect(result.current.choice).toEqual({ mode: "dark", light: "linen", dark: "basalt" });

    act(() => result.current.setMode("system"));
    act(() => result.current.setPalette("chalk"));
    expect(result.current.choice).toEqual({ mode: "system", light: "chalk", dark: "basalt" });
  });

  test("the quick switch shows the other side and keeps each side's palette", () => {
    const { result } = renderHook(() => useTheme());
    act(() => result.current.setPalette("basalt"));
    act(() => result.current.setMode("light"));
    act(() => result.current.toggle());
    expect(result.current.choice).toEqual({ mode: "dark", light: "linen", dark: "basalt" });
    expect([root.dataset.theme, root.dataset.scheme]).toEqual(["basalt", "dark"]);
    act(() => result.current.toggle());
    expect(result.current.choice.mode).toBe("light");

    // From "system", away from whatever the system shows now.
    act(() => result.current.setMode("system"));
    const shown = root.dataset.scheme;
    act(() => result.current.toggle());
    expect(result.current.choice.mode).toBe(shown === "light" ? "dark" : "light");
  });

  test("what does not fit — a theme since removed, a palette on the wrong side — is the default", () => {
    localStorage.setItem("kibo-theme", JSON.stringify({ mode: "light", light: "github-light", dark: "dark" }));
    expect(renderHook(() => useTheme()).result.current.choice.light).toBe("linen");
    localStorage.setItem("kibo-theme", JSON.stringify({ mode: "dark", light: "basalt", dark: "ember" }));
    expect(renderHook(() => useTheme()).result.current.choice).toEqual({ mode: "system", light: "linen", dark: "ember" });
  });
});
