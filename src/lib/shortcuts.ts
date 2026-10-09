import type { AsideTab } from "../types";

/**
 * A key and its modifiers. `code` is the physical key (`KeyB`, `Digit1`), not
 * the character: ⌘B has to work on a Russian layout, and Option turns
 * letters into other characters on a Mac. `mod` is ⌘ on a Mac and Ctrl
 * elsewhere; `ctrl` is the Control key everywhere.
 */
export type Combo = { code: string; mod?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean };

export type ShortcutGroup = "General" | "Panels" | "Chat" | "Files" | "Markdown viewer";
type Shortcut = { label: string; group: ShortcutGroup; combos: Combo[] };

// Every pane has one: `Record<AsideTab, …>` fails to compile when a pane is added without it.
const PANE_SHORTCUTS: Record<AsideTab, Shortcut> = {
  changes: { label: "Changes", group: "Panels", combos: [{ code: "Digit1", mod: true }] },
  plan: { label: "Plan", group: "Panels", combos: [{ code: "Digit2", mod: true }] },
  files: { label: "Files", group: "Panels", combos: [{ code: "Digit3", mod: true }] },
  rules: { label: "Rules", group: "Panels", combos: [{ code: "Digit4", mod: true }] },
  skills: { label: "Skills", group: "Panels", combos: [{ code: "Digit5", mod: true }] },
  mcp: { label: "MCP", group: "Panels", combos: [{ code: "Digit6", mod: true }] },
  hooks: { label: "Hooks", group: "Panels", combos: [{ code: "Digit7", mod: true }] },
  // As in VS Code.
  terminal: { label: "Terminal", group: "Panels", combos: [{ code: "Backquote", ctrl: true }] },
};

/**
 * Every key the app answers to, in the order the shortcuts dialog lists them.
 * A handler matches against its entry here rather than spelling the key out,
 * so the dialog cannot fall out of step with what the keys do.
 */
export const SHORTCUTS = {
  newChat: { label: "New chat", group: "General", combos: [{ code: "KeyN", mod: true }] },
  sidebar: { label: "Show or hide the sidebar", group: "General", combos: [{ code: "KeyB", mod: true }] },
  settings: { label: "Settings", group: "General", combos: [{ code: "Comma", mod: true }] },
  shortcuts: { label: "Keyboard shortcuts", group: "General", combos: [{ code: "Slash", mod: true }] },
  close: { label: "Close a dialog, menu, the file viewer or the search in a file", group: "General", combos: [{ code: "Escape" }] },
  ideLayout: { label: "IDE layout: files, the file and the chat side by side", group: "General", combos: [{ code: "KeyD", mod: true, shift: true }] },
  ...PANE_SHORTCUTS,
  focusInput: { label: "Go to the message box", group: "Chat", combos: [{ code: "KeyL", mod: true }] },
  send: { label: "Send the message", group: "Chat", combos: [{ code: "Enter" }] },
  newLine: { label: "New line in the message", group: "Chat", combos: [{ code: "Enter", shift: true }] },
  queue: { label: "Send the message after the current turn", group: "Chat", combos: [{ code: "Enter", alt: true }] },
  commandNext: { label: "Next command in the / menu", group: "Chat", combos: [{ code: "ArrowDown" }] },
  commandPrev: { label: "Previous command in the / menu", group: "Chat", combos: [{ code: "ArrowUp" }] },
  commandComplete: { label: "Complete the command's name", group: "Chat", combos: [{ code: "Tab" }] },
  closeFile: { label: "Close the file tab", group: "Files", combos: [{ code: "KeyE", mod: true }] },
  nextFile: { label: "Next changed file", group: "Files", combos: [{ code: "ArrowDown", alt: true }] },
  prevFile: { label: "Previous changed file", group: "Files", combos: [{ code: "ArrowUp", alt: true }] },
  save: { label: "Save the file or the config file", group: "Files", combos: [{ code: "KeyS", mod: true }] },
  find: { label: "Find and replace in the file", group: "Files", combos: [{ code: "KeyF", mod: true }] },
  findNext: { label: "Next match", group: "Files", combos: [{ code: "KeyG", mod: true }, { code: "F3" }] },
  findPrevious: { label: "Previous match", group: "Files", combos: [{ code: "KeyG", mod: true, shift: true }, { code: "F3", shift: true }] },
  textLarger: { label: "Larger text", group: "Markdown viewer", combos: [{ code: "Equal", mod: true }] },
  textSmaller: { label: "Smaller text", group: "Markdown viewer", combos: [{ code: "Minus", mod: true }] },
  textReset: { label: "Text at its usual size", group: "Markdown viewer", combos: [{ code: "Digit0", mod: true }] },
} satisfies Record<string, Shortcut>;

export type ShortcutId = keyof typeof SHORTCUTS;

export const IS_MAC = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

type KeyState = { code: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean };

/** Exact: ⌘⇧S is not ⌘S. */
export const matchesCombo = (e: KeyState, c: Combo, mac = IS_MAC) =>
  e.code === c.code &&
  e.metaKey === (mac && !!c.mod) &&
  e.ctrlKey === (!!c.ctrl || (!mac && !!c.mod)) &&
  e.shiftKey === !!c.shift &&
  e.altKey === !!c.alt;

/** Takes DOM and React keyboard events alike. */
export const matches = (e: KeyState, id: ShortcutId, mac = IS_MAC) =>
  SHORTCUTS[id].combos.some((c) => matchesCombo(e, c, mac));

const KEY_LABELS: Record<string, string> = {
  Backquote: "`",
  Comma: ",",
  Equal: "+",
  Minus: "−",
  Slash: "/",
  Escape: "Esc",
  ArrowDown: "↓",
  ArrowUp: "↑",
};

/** The keys to draw, in the platform's order: ⌃⌥⇧⌘ on a Mac, Ctrl+Alt+Shift elsewhere. */
export function comboKeys(c: Combo, mac = IS_MAC): string[] {
  const key = KEY_LABELS[c.code] ?? c.code.replace(/^(Key|Digit)/, "");
  const ctrl = c.ctrl || (!mac && c.mod);
  const mods = mac
    ? [ctrl && "⌃", c.alt && "⌥", c.shift && "⇧", c.mod && "⌘"]
    : [ctrl && "Ctrl", c.alt && "Alt", c.shift && "Shift"];
  return [...mods.filter((m): m is string => !!m), key];
}

/** One line for a menu row or a tooltip: `⌘1` on a Mac, `Ctrl+1` elsewhere. */
export const shortcutText = (id: ShortcutId, mac = IS_MAC) =>
  comboKeys(SHORTCUTS[id].combos[0], mac).join(mac ? "" : "+");
