import { useEffect, useState } from "react";

/**
 * The chat's text size, as a factor on the type scale (src/styles/tokens.css):
 * the chat counts from a base a step above the app's own — the size of a chat's
 * name in the sidebar, --fs-md, a little wider — and a step multiplies it, so
 * `medium` is that base.
 */
export const FONT_SIZES = { small: 0.92, medium: 1, large: 1.1, larger: 1.2 } as const;
export type FontSize = keyof typeof FONT_SIZES;

const KEY = "atlas-cli-chat-font-size";

export function useChatFontSize() {
  const [size, setSize] = useState<FontSize>(() => {
    const stored = localStorage.getItem(KEY);
    return stored && stored in FONT_SIZES ? (stored as FontSize) : "large";
  });

  useEffect(() => {
    localStorage.setItem(KEY, size);
    document.documentElement.style.setProperty("--chat-font-scale", String(FONT_SIZES[size]));
  }, [size]);

  return { size, setSize };
}
