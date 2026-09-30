import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import "./CopyAction.css";

/** Copies a message's text as written — Markdown source for an answer, not the rendered page. */
export function CopyAction({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <button
      type="button"
      className="copy-action"
      onClick={() => {
        // A clipboard that refuses loses nothing: the text is on screen.
        navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => setCopied(false), 1500);
        }, () => {});
      }}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
