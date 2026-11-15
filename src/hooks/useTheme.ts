import { useCallback, useEffect, useState } from "react";
import { setWindowGlass, setWindowTheme } from "../lib/window";

/** Each palette in src/styles/tokens.css, and whether it is light or dark. */
export const SCHEMES = {
  linen: "light",
  chalk: "light",
  clay: "light",
  tide: "light",
  ember: "dark",
  basalt: "dark",
  iris: "dark",
  fjord: "dark",
  frost: "dark",
} as const;
export type Theme = keyof typeof SCHEMES;
export type Scheme = (typeof SCHEMES)[Theme];

export const THEME_LABELS: Record<Theme, string> = {
  linen: "Linen",
  chalk: "Chalk",
  clay: "Clay",
  tide: "Tide",
  ember: "Ember",
  basalt: "Basalt",
  iris: "Iris",
  fjord: "Fjord",
  frost: "Frost",
};

/** The palettes on one side, in the order they are offered. */
export const themesOf = (scheme: Scheme) => (Object.keys(SCHEMES) as Theme[]).filter((t) => SCHEMES[t] === scheme);

export const MODES = ["system", "light", "dark"] as const;
export type Mode = (typeof MODES)[number];

/**
 * Light or dark — or whichever the system is set to — and the palette each
 * side draws with, so following the system does not mean the default palettes.
 */
export type ThemeChoice = { mode: Mode; light: Theme; dark: Theme };

const DEFAULT: ThemeChoice = { mode: "system", light: "linen", dark: "ember" };
const KEY = "kibo-theme";

const isMode = (value: unknown): value is Mode => (MODES as readonly unknown[]).includes(value);
const isTheme = (value: unknown): value is Theme => typeof value === "string" && value in SCHEMES;
const isChoice = (value: unknown): value is ThemeChoice => {
  if (typeof value !== "object" || value === null) return false;
  const { mode, light, dark } = value as Record<string, unknown>;
  return isMode(mode) && isTheme(light) && SCHEMES[light] === "light" && isTheme(dark) && SCHEMES[dark] === "dark";
};

/** What was stored, else the default. */
function stored(): ThemeChoice {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? "null");
    if (isChoice(value)) return value;
  } catch {
    // Storage that refuses, or a value that does not parse, leaves the default.
  }
  return DEFAULT;
}

/** Whether the system's glass goes under the window: Frost's, unless a scene covers it. */
export const glassUnder = (palette: Theme, sceneShown: boolean) => palette === "frost" && !sceneShown;

const systemIsLight = () => window.matchMedia("(prefers-color-scheme: light)").matches;

/** The side `choice` shows now. */
export const sideOf = (choice: ThemeChoice): Scheme =>
  choice.mode === "system" ? (systemIsLight() ? "light" : "dark") : choice.mode;

/**
 * Resolves the choice to a palette and writes it to <html data-theme>, which is
 * what src/styles/tokens.css keys its palettes off, and its side to
 * `data-scheme`, for what comes only in light and dark (the code colours). A
 * new palette there becomes selectable by adding it to SCHEMES.
 * `sceneShown`: a scene is chosen, which under Frost covers the system glass.
 */
export function useTheme(sceneShown = false) {
  const [choice, setChoice] = useState<ThemeChoice>(stored);

  useEffect(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify(choice));
    } catch {
      // Only the next launch's theme is lost.
    }
    const apply = () => {
      const side = sideOf(choice);
      document.documentElement.dataset.theme = choice[side];
      document.documentElement.dataset.scheme = side;
      // Frost is see-through: the system's blur goes under the window only
      // while it shows, since the other palettes paint over it anyway.
      void setWindowGlass(glassUnder(choice[side], sceneShown));
    };
    apply();
    // The window's edge in the app's side, not the system's. Under "system"
    // it follows the system — pinned, the webview would stop seeing it change.
    void setWindowTheme(choice.mode === "system" ? null : sideOf(choice));
    if (choice.mode !== "system") return;
    const mql = window.matchMedia("(prefers-color-scheme: light)");
    mql.addEventListener("change", apply);
    return () => mql.removeEventListener("change", apply);
  }, [choice, sceneShown]);

  const setMode = useCallback((mode: Mode) => setChoice((c) => ({ ...c, mode })), []);
  // Picked while the other side shows, a palette switches to its own side — a
  // click that changed nothing on screen would read as broken. Under "system"
  // the system keeps deciding the side.
  const setPalette = useCallback(
    (theme: Theme) =>
      setChoice((c) => {
        const side = SCHEMES[theme];
        return { ...c, [side]: theme, mode: c.mode === "system" ? "system" : side };
      }),
    [],
  );

  // The other side, fixed: from "system" too, since the point is to leave
  // what is on screen. The palettes stay as Settings has them.
  const toggle = useCallback(() => setChoice((c) => ({ ...c, mode: sideOf(c) === "light" ? "dark" : "light" })), []);

  return { choice, setMode, setPalette, toggle };
}
