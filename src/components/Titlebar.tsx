import type { MouseEvent, ReactNode, Ref } from "react";
import { WindowControls } from "./WindowControls";
import { useWindowFullscreen } from "../hooks/useWindowFullscreen";
import { nativeFrame, startWindowDrag, toggleMaximizeWindow, windowsFrame } from "../lib/window";
import "./Titlebar.css";

// Titlebar drag: single press drags the window, double press zooms it — the macOS
// titlebar contract, driven explicitly so clicks on the controls stay clicks.
export const dragOrMaximize = (e: MouseEvent) => {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
  // Without this the webview keeps extending a text selection while the window
  // moves under the cursor, which flickers through whatever it passes over.
  e.preventDefault();
  if (e.detail === 2) toggleMaximizeWindow();
  else startWindowDrag();
};

/**
 * The window's controls and its title, centred; the bar is the window's drag
 * handle. `actions` sit at the right — left of Windows' caption buttons, which
 * keep the edge. `leading` follows the traffic lights — at the left edge where
 * there are none: Windows, or macOS full screen, which takes them away. `slot`
 * is the room between them, where the chat draws its header (a portal: it
 * drags the window itself, since its events bubble through the chat, not
 * through this bar).
 */
export function Titlebar({
  children,
  leading,
  actions,
  slot,
}: {
  children?: ReactNode;
  leading?: ReactNode;
  actions?: ReactNode;
  slot?: Ref<HTMLDivElement>;
}) {
  const fullscreen = useWindowFullscreen(nativeFrame);
  return (
    <div className={`titlebar${nativeFrame ? " native" : ""}`} onMouseDown={dragOrMaximize}>
      {!windowsFrame && !fullscreen && <WindowControls />}
      {leading}
      {children && <span className="titlebar-title">{children}</span>}
      {slot && <div className="titlebar-slot" ref={slot} />}
      {actions && <div className="titlebar-actions">{actions}</div>}
      {windowsFrame && <WindowControls />}
    </div>
  );
}
