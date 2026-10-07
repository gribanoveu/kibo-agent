import { useEffect, useState } from "react";
import { onMaximizedChange } from "../lib/window";

/** Whether the window is maximized, followed only while `enabled`. */
export function useWindowMaximized(enabled: boolean): boolean {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => (enabled ? onMaximizedChange(setMaximized) : undefined), [enabled]);
  return maximized;
}
