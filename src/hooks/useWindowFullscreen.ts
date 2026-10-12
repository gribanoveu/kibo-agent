import { useEffect, useState } from "react";
import { onFullscreenChange } from "../lib/window";

/** Whether the window is full screen, followed only while `enabled`. */
export function useWindowFullscreen(enabled: boolean): boolean {
  const [fullscreen, setFullscreen] = useState(false);
  useEffect(() => (enabled ? onFullscreenChange(setFullscreen) : undefined), [enabled]);
  return fullscreen;
}
