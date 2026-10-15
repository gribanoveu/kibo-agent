import { useCallback, useEffect, useRef, type RefObject } from "react";
import { openExternal } from "../lib/window";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { pasteAtPrompt } from "../lib/pasteAtPrompt";
import { terminalAttach, terminalResize, terminalWrite } from "../lib/terminal";
import "@xterm/xterm/css/xterm.css";

/**
 * How long the size has to hold still before the shell is told it. xterm.js
 * follows the element every frame of a drag, a window resize or the dock's
 * width transition; were each of those widths a SIGWINCH, zsh would redraw
 * its prompt for a width xterm.js had already left, and the misplaced copies
 * would stay on screen. Told once, when xterm.js is already at the final
 * width, the shell redraws at the width it is shown at.
 */
const SETTLE_MS = 150;

const ANSI = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

/**
 * The terminal's look, from the theme's tokens: xterm.js draws its own cells
 * and takes colours as values, not as CSS. The font is whatever the element's
 * CSS resolves `--font-mono` and `--fs-sm` to.
 */
function look(element: HTMLElement) {
  const root = getComputedStyle(document.documentElement);
  const token = (name: string) => root.getPropertyValue(name).trim();
  const theme: ITheme = {
    background: token("--bg-panel"),
    foreground: token("--text"),
    cursor: token("--text"),
    cursorAccent: token("--bg-panel"),
    selectionBackground: token("--bg-active"),
  };
  for (const name of ANSI) theme[name] = token(`--term-${name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`);
  const css = getComputedStyle(element);
  return { theme, fontFamily: css.fontFamily, fontSize: parseFloat(css.fontSize) || 12 };
}

/**
 * Draws terminal `id` into `container`: what it wrote so far, then what it
 * writes; what is typed goes to its shell, and its size follows the element's.
 * Detached when it unmounts — the shell runs on in the backend. What the user
 * selects is told to `onSelection` ("" when nothing is).
 */
export function useTerminalScreen(
  id: number,
  container: RefObject<HTMLDivElement | null>,
  onSelection: (text: string) => void,
) {
  const shown = useRef<Terminal | null>(null);
  // The latest callback, without drawing the terminal again when it changes.
  const selected = useRef(onSelection);
  selected.current = onSelection;

  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const term = new Terminal({ ...look(element), cursorBlink: true, scrollback: 5000 });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon((_, uri) => void openExternal(uri)));
    term.open(element);
    shown.current = term;

    const input = term.onData((data) => void terminalWrite(id, data));
    // What an answer asked to put in this shell (`queuePaste`), at its prompt.
    const stopPasting = pasteAtPrompt(term, id);
    const selection = term.onSelectionChange(() => selected.current(term.getSelection()));
    let settling: ReturnType<typeof setTimeout> | undefined;
    const resized = term.onResize(({ cols, rows }) => {
      clearTimeout(settling);
      settling = setTimeout(() => void terminalResize(id, cols, rows), SETTLE_MS);
    });
    // Hidden or not laid out yet, there is nothing to measure.
    const refit = () => {
      if (element.clientWidth > 0 && element.clientHeight > 0) fit.fit();
    };
    refit();
    // Fitting to the size xterm.js already had says nothing; the shell may
    // still be at the size the last screen gave it.
    void terminalResize(id, term.cols, term.rows);
    const sized = new ResizeObserver(refit);
    sized.observe(element);
    // A theme or font-size change is an attribute on <html>.
    const restyled = new MutationObserver(() => {
      const next = look(element);
      term.options.theme = next.theme;
      term.options.fontFamily = next.fontFamily;
      term.options.fontSize = next.fontSize;
      refit();
    });
    restyled.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "style"] });

    let live = true;
    let detach: (() => void) | undefined;
    terminalAttach(id, (bytes) => term.write(bytes)).then(
      (off) => (live ? (detach = off) : off()),
      (e) => term.write(`\r\n${String(e)}\r\n`),
    );

    // Opened from the pane — its "+", a tab — or with nothing else focused:
    // typing goes here. Not taken from the composer the user is typing in.
    const focused = document.activeElement;
    if (!focused || focused === document.body || element.closest(".aside")?.contains(focused)) term.focus();

    return () => {
      live = false;
      detach?.();
      shown.current = null;
      stopPasting();
      input.dispose();
      selection.dispose();
      resized.dispose();
      clearTimeout(settling);
      sized.disconnect();
      restyled.disconnect();
      term.dispose();
    };
  }, [id, container]);

  return { clearSelection: useCallback(() => shown.current?.clearSelection(), []) };
}
