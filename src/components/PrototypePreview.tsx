import { useEffect, useRef, useState } from "react";
import { AppWindow, FolderOpen, RotateCw, Wrench, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import type { PrototypePreview as Preview } from "../hooks/usePrototypePreview";
import { revealPrototype } from "../lib/prototype";
import "./PrototypePreview.css";

type Props = {
  preview: Preview;
  chatId: string;
  onNotify: (message: string) => void;
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
export function PrototypePreview({ preview, chatId, onNotify }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [errors, setErrors] = useState<string[]>([]);

  // Each drawing of the page reports its own; a frame drawn anew starts clean.
  useEffect(() => {
    setErrors([]);
    const heard = (e: MessageEvent) => {
      // Only this frame's, and only what the reporter sends.
      if (e.source !== frame.current?.contentWindow) return;
      const data = e.data as { kibo?: unknown; message?: unknown } | null;
      if (data?.kibo !== "page-error" || typeof data.message !== "string") return;
      const message = data.message;
      setErrors((was) => (was.includes(message) ? was : [...was, message].slice(-3)));
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

  return (
    <aside className="prototype-preview">
      <div className="prototype-preview-head">
        <h2 className="prototype-preview-title">
          <AppWindow size={14} />
          Preview
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
        <button type="button" className="iconbtn" title="Reload the page" onClick={preview.refresh}>
          <RotateCw size={13} />
        </button>
        <button
          type="button"
          className="iconbtn"
          title="Show the chat's folder"
          onClick={() => revealPrototype(chatId).catch((e) => onNotify(String(e)))}
        >
          <FolderOpen size={13} />
        </button>
        <button type="button" className="iconbtn" title="Close the preview" onClick={preview.close}>
          <X size={14} />
        </button>
      </div>
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
    </aside>
  );
}
