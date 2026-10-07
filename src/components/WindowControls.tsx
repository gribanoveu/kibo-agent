import { useWindowMaximized } from "../hooks/useWindowMaximized";
import { closeWindow, minimizeWindow, nativeFrame, toggleMaximizeWindow, windowsFrame } from "../lib/window";
import "./WindowControls.css";

export function WindowControls({ windows = windowsFrame }: { windows?: boolean }) {
  const maximized = useWindowMaximized(windows);
  if (windows) {
    // Windows' caption buttons: wide, square, at the right edge of the bar.
    return (
      <div className="window-controls windows">
        <button className="caption" type="button" title="Minimize" aria-label="Minimize" onClick={minimizeWindow}>
          <svg viewBox="0 0 10 10" aria-hidden="true">
            <path d="M0 5h10" />
          </svg>
        </button>
        <button
          className="caption"
          type="button"
          title={maximized ? "Restore" : "Maximize"}
          aria-label={maximized ? "Restore" : "Maximize"}
          onClick={toggleMaximizeWindow}
        >
          <svg viewBox="0 0 10 10" aria-hidden="true">
            {maximized ? <path d="M.5 2.5h7v7h-7zM2.5 2.5v-2h7v7h-2" /> : <path d="M.5.5h9v9h-9z" />}
          </svg>
        </button>
        <button className="caption close" type="button" title="Close" aria-label="Close" onClick={closeWindow}>
          <svg viewBox="0 0 10 10" aria-hidden="true">
            <path d="M0 0l10 10M10 0L0 10" />
          </svg>
        </button>
      </div>
    );
  }
  return (
    // Under the native traffic lights the buttons only hold their place.
    <div className={`window-controls${nativeFrame ? " native" : ""}`}>
      <button className="dot r" type="button" title="Close" aria-label="Close" onClick={closeWindow}>
        <svg viewBox="0 0 10 10" aria-hidden="true">
          <path d="M3 3l4 4M7 3l-4 4" />
        </svg>
      </button>
      <button
        className="dot y"
        type="button"
        title="Minimize"
        aria-label="Minimize"
        onClick={minimizeWindow}
      >
        <svg viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2.6 5h4.8" />
        </svg>
      </button>
      <button
        className="dot g"
        type="button"
        title="Maximize"
        aria-label="Maximize"
        onClick={toggleMaximizeWindow}
      >
        <svg viewBox="0 0 10 10" aria-hidden="true">
          <path d="M5 2.4v5.2M2.4 5h5.2" />
        </svg>
      </button>
    </div>
  );
}
