import { useEffect, useRef, useState } from "react";
import { AppWindow, Download, FolderOpen, Images, Maximize2, Minimize2, MousePointerClick, RotateCcw, RotateCw, Wrench, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { PrototypeAssets } from "./PrototypeAssets";
import type { PrototypePreview as Preview } from "../hooks/usePrototypePreview";
import { pickSavePath } from "../lib/dialog";
import { exportPrototype, pointedOf, revealPrototype } from "../lib/prototype";
import "./PrototypePreview.css";

type Props = {
  preview: Preview;
  chatId: string;
  onNotify: (message: string) => void;
  /** Puts the folder back to the version shown; absent while a turn is under way. */
  onRestore?: (version: number) => void;
};

/** The working folder as it is now, in the picker beside the versions. */
const NOW = "now";

/**
 * The Preview panel in the column right of a Prototype chat: the page in a
 * sandboxed frame (docs/31-prototype-role.md, decision 4) — scripts run, and
 * nothing else: no same origin, no leaving the frame, no windows of its own.
 * What goes wrong on the page arrives as a message (`prototype_protocol`'s
 * reporter) and is listed under it, to hand to the model.
 */
export function PrototypePreview({ preview, chatId, onNotify, onRestore }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [errors, setErrors] = useState<string[]>([]);
  // The pointer is on in the page: the next click there names an element.
  const [picking, setPicking] = useState(false);
  // What the listener below reads as it is now, not as it was when the frame was drawn.
  const now = useRef({ picking, page: preview.page?.path, point: preview.point });
  now.current = { picking, page: preview.page?.path, point: preview.point };

  // Each drawing of the page reports its own; a frame drawn anew starts clean,
  // its pointer off.
  useEffect(() => {
    setErrors([]);
    setPicking(false);
    const heard = (e: MessageEvent) => {
      // Only this frame's, and only what the panel's scripts send.
      if (e.source !== frame.current?.contentWindow) return;
      const data = e.data as { kibo?: unknown; message?: unknown } | null;
      if (data?.kibo === "page-error" && typeof data.message === "string") {
        const message = data.message;
        setErrors((was) => (was.includes(message) ? was : [...was, message].slice(-3)));
      }
      // An element only while the user asked for one: the page cannot put one there itself.
      if (data?.kibo === "picked" && now.current.picking && now.current.page) {
        const pointed = pointedOf(data, now.current.page);
        if (pointed) now.current.point(pointed);
        setPicking(false);
      }
      if (data?.kibo === "pick-cancelled") setPicking(false);
    };
    addEventListener("message", heard);
    return () => removeEventListener("message", heard);
  }, [preview.key]);

  const options = [
    { value: NOW, label: "Now", hint: "The folder as it is" },
    ...preview.versions
      .slice()
      .reverse()
      .map((v) => ({ value: `${v.version}`, label: `Version ${v.version}`, hint: v.title })),
  ];
  const value = preview.shown === null ? NOW : `${preview.shown}`;

  // On the folder as it is only: an element of an older version may be gone from it.
  const pointer = () => {
    frame.current?.contentWindow?.postMessage({ kibo: picking ? "pick-stop" : "pick" }, "*");
    setPicking(!picking);
  };

  // One file, the libraries in it, of what the panel shows now.
  const saveAs = async () => {
    const page = preview.page;
    if (!page) return;
    const path = await pickSavePath(page.title, "html");
    if (!path) return;
    try {
      await exportPrototype(chatId, preview.shown, page.path, path);
      onNotify(`Saved to ${path.split(/[\\/]/).pop()}`);
    } catch (e) {
      onNotify(String(e));
    }
  };

  return (
    <aside className="prototype-preview">
      <div className="prototype-preview-head">
        <h2 className="prototype-preview-title">
          <AppWindow size={14} />
          <span className="prototype-preview-title-text">Preview</span>
        </h2>
        <Dropdown
          title="Which version the panel shows"
          heading="Show"
          label={value === NOW ? "Now" : `Version ${value}`}
          options={options}
          value={value}
          onPick={(picked) => preview.show(picked === NOW ? null : Number(picked))}
          below
          right
        />
        {preview.shown !== null && (
          <button
            type="button"
            className="prototype-preview-restore"
            title={onRestore ? "Make the folder this version again — kept as a new version, nothing is lost" : "Wait for the turn to finish"}
            disabled={!onRestore}
            onClick={() => preview.shown !== null && onRestore?.(preview.shown)}
          >
            <RotateCcw size={12} />
            <span className="prototype-preview-restore-label">Restore</span>
          </button>
        )}
        {preview.shown === null && (
          <button
            type="button"
            className={`iconbtn${picking ? " on" : ""}`}
            title={picking ? "Stop pointing (Escape on the page)" : "Point at an element to ask for a change to it alone"}
            aria-pressed={picking}
            onClick={pointer}
          >
            <MousePointerClick size={13} />
          </button>
        )}
        <button
          type="button"
          className={`iconbtn prototype-preview-assets${preview.assetsOpen ? " on" : ""}`}
          title={preview.assetsOpen ? "Hide the pictures" : "Show the pictures in the chat's folder"}
          aria-pressed={preview.assetsOpen}
          onClick={preview.toggleAssets}
        >
          <Images size={13} />
          {(preview.assets?.assets.length ?? 0) > 0 && <span>{preview.assets?.assets.length}</span>}
        </button>
        <button type="button" className="iconbtn" title="Reload the page" onClick={preview.refresh}>
          <RotateCw size={13} />
        </button>
        <button type="button" className="iconbtn" title="Save as one HTML file, for any browser" onClick={saveAs}>
          <Download size={13} />
        </button>
        <button
          type="button"
          className="iconbtn"
          title="Show the chat's folder"
          onClick={() => revealPrototype(chatId).catch((e) => onNotify(String(e)))}
        >
          <FolderOpen size={13} />
        </button>
        <button
          type="button"
          className="iconbtn"
          title={preview.expanded ? "Back beside the chat" : "Expand over the chat"}
          aria-pressed={preview.expanded}
          onClick={preview.toggleExpanded}
        >
          {preview.expanded ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
        </button>
        <button type="button" className="iconbtn" title="Close the preview" onClick={preview.close}>
          <X size={14} />
        </button>
      </div>
      {!preview.url && <p className="prototype-preview-empty">No page yet — ask for one, and it is drawn here.</p>}
      {preview.url && (
        <iframe
          key={preview.key}
          ref={frame}
          className="prototype-preview-frame"
          title="Preview of the page"
          src={preview.url}
          sandbox="allow-scripts allow-modals allow-forms"
          allow="fullscreen"
        />
      )}
      {errors.length > 0 && (
        <div className="prototype-preview-errors">
          {errors.map((message) => (
            <div key={message} className="prototype-preview-error">
              <span className="prototype-preview-error-text">{message}</span>
              <button
                type="button"
                className="prototype-preview-fix"
                title="Put this error in the message box"
                onClick={() => preview.askFix(`The page shows an error: ${message}. Fix it.`)}
              >
                <Wrench size={12} />
                Fix
              </button>
            </div>
          ))}
        </div>
      )}
      <PrototypeAssets preview={preview} chatId={chatId} onNotify={onNotify} />
    </aside>
  );
}
