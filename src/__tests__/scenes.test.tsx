import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook } from "@testing-library/react";
import { PAUSED, SCENE_CSP, sceneDocument, shouldPause } from "../lib/sceneDocument";
import { BUILTIN_SCENES } from "../lib/scenes";
import { useScene } from "../hooks/useScene";
import { glassUnder, useTheme } from "../hooks/useTheme";
import { WindowScene } from "../components/WindowScene";

// A scene is HTML and CSS drawn behind Frost's glass (docs/55-frost-scenes.md).
// The frame runs no script; the document's own policy keeps it off the
// network; it pauses while the window is not in use.

// What the theme asks of the system's glass. The rest of the module stays the
// real one, for every file that loads it after this one.
const glass: boolean[] = [];
const realWindow = await import("../lib/window");
mock.module("../lib/window", () => ({
  ...realWindow,
  setWindowGlass: (on: boolean) => {
    glass.push(on);
    return false;
  },
}));

// Outside Tauri: the window's focus comes from the page's own events. A file
// run before this one may leave the app's marker on `window`.
const hasFocus = document.hasFocus;
const host = window as unknown as Record<string, unknown>;
let tauri: unknown;
beforeEach(() => {
  tauri = host.__TAURI_INTERNALS__;
  delete host.__TAURI_INTERNALS__;
});
afterEach(() => {
  localStorage.clear();
  document.hasFocus = hasFocus;
  if (tauri !== undefined) host.__TAURI_INTERNALS__ = tauri;
});

const parse = (html: string) => new DOMParser().parseFromString(html, "text/html");

describe("a scene's document", () => {
  test("opens with the policy, before anything of the scene's", () => {
    const sneaky = `</head><meta http-equiv="Content-Security-Policy" content="default-src *"><img src="https://x.test/p.png">`;
    const html = sceneDocument(sneaky);
    const first = parse(html).head.firstElementChild;
    expect(first?.getAttribute("http-equiv")).toBe("Content-Security-Policy");
    expect(first?.getAttribute("content")).toBe(SCENE_CSP);
    expect(html.indexOf(SCENE_CSP)).toBeLessThan(html.indexOf(sneaky));
  });

  test("the policy lets in styles and inline pictures, and nothing from anywhere", () => {
    expect(SCENE_CSP).toContain("default-src 'none'");
    expect(SCENE_CSP).toContain("style-src 'unsafe-inline'");
    expect(SCENE_CSP).not.toMatch(/https?:|\*/);
  });

  test("stands still where the system asks for less motion, and wherever it is paused", () => {
    const style = parse(sceneDocument("<i></i>")).head.querySelector("style")?.textContent ?? "";
    // Held where it is, not stopped: a scene built on negative delays shows its start then.
    expect(style).toMatch(/@media \(prefers-reduced-motion: reduce\)\{\*,\*::before,\*::after\{animation-play-state:paused!important/);
    expect(style).not.toContain("animation:none");
    expect(style).toContain(`html.${PAUSED} *,`);
    expect(style).toContain("animation-play-state:paused!important");
    expect(parse(sceneDocument("<i></i>", true)).documentElement.classList.contains(PAUSED)).toBe(true);
    expect(parse(sceneDocument("<i></i>")).documentElement.classList.contains(PAUSED)).toBe(false);
  });

  test("pauses while hidden, and out of focus unless asked to keep moving", () => {
    expect(shouldPause(false, true, false)).toBe(false);
    expect(shouldPause(false, false, false)).toBe(true);
    expect(shouldPause(false, false, true)).toBe(false);
    expect(shouldPause(true, true, true)).toBe(true);
  });
});

/** Every property a scene's keyframes animate. */
function animated(body: string): Set<string> {
  const props = new Set<string>();
  for (const [, frames] of body.matchAll(/@keyframes[^{]*\{((?:[^{}]*\{[^{}]*\})*)\}/g)) {
    for (const [, prop] of frames.matchAll(/([a-z-]+)\s*:/g)) props.add(prop);
  }
  return props;
}

describe("the scenes the app ships", () => {
  test("have names of their own", () => {
    expect(new Set(BUILTIN_SCENES.map((s) => s.id)).size).toBe(BUILTIN_SCENES.length);
  });

  for (const scene of BUILTIN_SCENES) {
    test(`${scene.title} moves only what the compositor moves, and reaches for nothing`, () => {
      const body = scene.body();
      const props = animated(body);
      expect(props.size).toBeGreaterThan(0);
      expect([...props].filter((p) => p !== "transform" && p !== "opacity")).toEqual([]);
      expect(body).not.toMatch(/<script|\son[a-z]+=|https?:|<meta|<link|<iframe/i);
      // The same every time it is built: a card's picture is the window's.
      expect(scene.body()).toBe(body);
    });
  }

  test("the keyframes check sees a property it must refuse", () => {
    expect(animated("@keyframes a{from{filter:blur(0)}to{filter:blur(4px)}}")).toEqual(new Set(["filter"]));
  });
});

describe("the chosen scene", () => {
  test("is none at first, and moves only while the window is in use", () => {
    const { result } = renderHook(() => useScene());
    expect(result.current.active).toBeNull();
    expect(result.current.body).toBeNull();
    expect(result.current.animateInBackground).toBe(false);
  });

  test("a scene the app ships is drawn, and kept for the next launch", () => {
    const { result } = renderHook(() => useScene());
    act(() => result.current.choose({ kind: "builtin", id: "snowfall" }));
    expect(result.current.active).toEqual({ kind: "builtin", id: "snowfall" });
    expect(result.current.body).toBe(BUILTIN_SCENES.find((s) => s.id === "snowfall")!.body());
    expect(JSON.parse(localStorage.getItem("kibo-scene") ?? "null")).toEqual({ kind: "builtin", id: "snowfall" });
  });

  test("one that names nothing there now, or a stored value that does not fit, is no scene", () => {
    localStorage.setItem("kibo-scene", JSON.stringify({ kind: "builtin", id: "gone" }));
    expect(renderHook(() => useScene()).result.current).toMatchObject({ active: null, body: null });
    localStorage.setItem("kibo-scene", JSON.stringify({ kind: "painted", id: 3 }));
    expect(renderHook(() => useScene()).result.current).toMatchObject({ active: null, body: null });
  });
});

describe("the window's glass", () => {
  test("goes under Frost, but not under a scene or another palette", () => {
    expect(glassUnder("frost", false)).toBe(true);
    expect(glassUnder("frost", true)).toBe(false);
    expect(glassUnder("ember", false)).toBe(false);
  });

  test("the theme takes it away when a scene shows, and gives it back", () => {
    const { result, rerender } = renderHook(({ scene }) => useTheme(scene), { initialProps: { scene: false } });
    act(() => result.current.setMode("dark"));
    act(() => result.current.setPalette("frost"));
    expect(glass.at(-1)).toBe(true);
    rerender({ scene: true });
    expect(glass.at(-1)).toBe(false);
    rerender({ scene: false });
    expect(glass.at(-1)).toBe(true);
  });
});

describe("the scene behind the window", () => {
  // The frame's load and the blur's deferred check, both settled.
  const settle = () => act(() => new Promise<void>((done) => setTimeout(done, 1)));
  const focus = (has: boolean) => {
    document.hasFocus = () => has;
  };

  test("runs in a frame without scripts, on the app's document", async () => {
    const { container } = render(<WindowScene body="<i></i>" animateInBackground={false} />);
    await settle();
    const frame = container.querySelector("iframe")!;
    expect(frame.getAttribute("sandbox")).toBe("allow-same-origin");
    expect(frame.getAttribute("srcdoc")).toBe(sceneDocument("<i></i>"));
    expect(frame.tabIndex).toBe(-1);
    expect(container.firstElementChild?.getAttribute("aria-hidden")).toBe("true");
  });

  test("pauses when the window loses focus and moves again when it is back", async () => {
    focus(true);
    const { container } = render(<WindowScene body="<i></i>" animateInBackground={false} />);
    await settle();
    const root = () => (container.querySelector("iframe") as HTMLIFrameElement).contentDocument!.documentElement;
    expect(root().classList.contains(PAUSED)).toBe(false);

    focus(false);
    fireEvent.blur(window);
    await settle();
    expect(root().classList.contains(PAUSED)).toBe(true);

    focus(true);
    fireEvent.focus(window);
    expect(root().classList.contains(PAUSED)).toBe(false);
  });

  test("asked to keep moving, ignores the blur but not a hidden window", async () => {
    focus(false);
    const { container } = render(<WindowScene body="<i></i>" animateInBackground />);
    await settle();
    const root = () => (container.querySelector("iframe") as HTMLIFrameElement).contentDocument!.documentElement;
    fireEvent.blur(window);
    await settle();
    expect(root().classList.contains(PAUSED)).toBe(false);

    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    try {
      fireEvent(document, new Event("visibilitychange"));
      expect(root().classList.contains(PAUSED)).toBe(true);
    } finally {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    }
  });
});
