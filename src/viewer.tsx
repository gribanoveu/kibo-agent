// A Markdown file opened with the app, in a window of its own
// (src-tauri/src/commands/viewer.rs). Only the Markdown and the theme come
// along: none of App's hooks, so no folder, index or agent starts for it.
import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "./styles/tokens.css";
import "./viewer.css";
import { Markdown } from "./components/Markdown";
import { Titlebar } from "./components/Titlebar";
import { useTheme } from "./hooks/useTheme";
import { useChatFontSize } from "./hooks/useChatFontSize";
import { nativeFrame, nativeMenu, viewerFile, windowsFrame } from "./lib/window";

document.addEventListener("contextmenu", (e) => nativeMenu(e) || e.preventDefault());

function Viewer() {
  useTheme();
  useChatFontSize();
  const [file, setFile] = useState<{ name: string; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    viewerFile().then(setFile, (e: unknown) => setError(String(e)));
  }, []);
  return (
    <div className={`viewer${nativeFrame || windowsFrame ? " framed" : ""}`}>
      <Titlebar>{file?.name}</Titlebar>
      <main className="viewer-scroll chat-text">
        <article className="viewer-page">
          {error !== null ? (
            <p className="viewer-error">{error}</p>
          ) : (
            file !== null && <Markdown text={file.text} streaming={false} />
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
