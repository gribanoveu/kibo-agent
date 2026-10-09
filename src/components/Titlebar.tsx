import type { MouseEvent, ReactNode } from "react";
import { WindowControls } from "./WindowControls";
import { nativeFrame, startWindowDrag, toggleMaximizeWindow } from "../lib/window";
import "./Titlebar.css";

// Titlebar drag: single press drags the window, double press zooms it — the macOS
// titlebar contract, driven explicitly so clicks on the controls stay clicks.
const dragOrMaximize = (e: MouseEvent) => {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
  // Without this the webview keeps extending a text selection while the window
  // moves under the cursor, which flickers through whatever it passes over.
  e.preventDefault();
  if (e.detail === 2) toggleMaximizeWindow();
  else startWindowDrag();
};

/** The window's controls and its title, centred; the bar is the window's drag handle. */
export function Titlebar({ children }: { children: ReactNode }) {
  return (
    <div className={`titlebar${nativeFrame ? " native" : ""}`} onMouseDown={dragOrMaximize}>
      <WindowControls />
      <span className="titlebar-title">{children}</span>
    </div>
  );
}
