// A Markdown file opened with the app, in a window of its own
// (src-tauri/src/commands/viewer.rs). Only the Markdown and the theme come
// along: none of App's hooks, so no folder, index or agent starts for it.
import React, { useEffect, useState, type CSSProperties } from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles/tokens.css";
import "./viewer.css";
import { Markdown } from "./components/Markdown";
import { Titlebar } from "./components/Titlebar";
import { useTheme } from "./hooks/useTheme";
import { AArrowDown, AArrowUp } from "lucide-react";
import { useShortcuts } from "./hooks/useShortcuts";
import { useViewerTextSize } from "./hooks/useViewerTextSize";
import { shortcutText } from "./lib/shortcuts";
import { nativeFrame, nativeMenu, viewerFile, windowsFrame } from "./lib/window";

document.addEventListener("contextmenu", (e) => nativeMenu(e) || e.preventDefault());

function Viewer() {
  useTheme();
  const size = useViewerTextSize();
  useShortcuts({ textLarger: size.larger, textSmaller: size.smaller, textReset: size.reset });
  const [file, setFile] = useState<{ name: string; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    viewerFile().then(setFile, (e: unknown) => setError(String(e)));
  }, []);
  return (
    <div className={`viewer${nativeFrame || windowsFrame ? " framed" : ""}`}>
      <Titlebar
        actions={
          <>
            <button type="button" className="viewer-size" title={`Smaller text (${shortcutText("textSmaller")})`} aria-label="Smaller text" onClick={size.smaller}>
              <AArrowDown size={15} aria-hidden />
            </button>
            <button type="button" className="viewer-size percent" title={`Text at its usual size (${shortcutText("textReset")})`} onClick={size.reset}>
              {size.scale !== null && `${Math.round(size.scale * 100)}%`}
            </button>
            <button type="button" className="viewer-size" title={`Larger text (${shortcutText("textLarger")})`} aria-label="Larger text" onClick={size.larger}>
              <AArrowUp size={15} aria-hidden />
            </button>
          </>
        }
      >
        {file?.name}
      </Titlebar>
      <main className="viewer-scroll chat-text" style={{ "--chat-font-scale": size.scale ?? undefined } as CSSProperties}>
        <article className="viewer-page">
          {error !== null ? (
            <p className="viewer-error">{error}</p>
          ) : (
            file !== null && size.scale !== null && <Markdown text={file.text} streaming={false} />
          )}
        </article>
      </main>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Viewer />
  </React.StrictMode>,
);
