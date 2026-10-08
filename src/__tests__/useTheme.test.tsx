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
    act(() => result.current.setPalette("latte"));
    expect([root.dataset.theme, root.dataset.scheme]).toEqual(["latte", "light"]);
    act(() => result.current.setMode("dark"));
    expect([root.dataset.theme, root.dataset.scheme]).toEqual(["dark", "dark"]);
    // Kept for when light is back.
    expect(result.current.choice.light).toBe("latte");
  });

  test("a palette of the other side switches to it, unless the system decides", () => {
    const { result } = renderHook(() => useTheme());
    act(() => result.current.setMode("light"));
    act(() => result.current.setPalette("one-dark"));
    expect(result.current.choice).toEqual({ mode: "dark", light: "light", dark: "one-dark" });

    act(() => result.current.setMode("system"));
    act(() => result.current.setPalette("latte"));
    expect(result.current.choice).toEqual({ mode: "system", light: "latte", dark: "one-dark" });
  });

  test("the quick switch shows the other side and keeps each side's palette", () => {
    const { result } = renderHook(() => useTheme());
    act(() => result.current.setPalette("one-dark"));
    act(() => result.current.setMode("light"));
    act(() => result.current.toggle());
    expect(result.current.choice).toEqual({ mode: "dark", light: "light", dark: "one-dark" });
    expect([root.dataset.theme, root.dataset.scheme]).toEqual(["one-dark", "dark"]);
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
    expect(renderHook(() => useTheme()).result.current.choice.light).toBe("light");
    localStorage.setItem("kibo-theme", JSON.stringify({ mode: "dark", light: "one-dark", dark: "dark" }));
    expect(renderHook(() => useTheme()).result.current.choice).toEqual({ mode: "system", light: "light", dark: "dark" });
  });
});
