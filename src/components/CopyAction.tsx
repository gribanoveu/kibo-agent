import { Check, Copy } from "lucide-react";
import { useCopied } from "../hooks/useCopied";
import "./CopyAction.css";

/** Copies a message's text as written — Markdown source for an answer, not the rendered page. */
export function CopyAction({ text }: { text: string }) {
  const { copied, copy } = useCopied();
  return (
    <button type="button" className="copy-action" onClick={() => copy(text)}>
      {copied ? <Check size={12} /> : <Copy size={12} />}
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
