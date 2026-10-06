import { useEffect, useMemo, useState, type MouseEvent } from "react";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import { parseFragment } from "parse5";
import { fromParse5 } from "hast-util-from-parse5";
import { defaultSchema, sanitize, type Schema } from "hast-util-sanitize";
import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import { openUrl } from "@tauri-apps/plugin-opener";
import { asciidocHtml } from "../lib/asciidoc";
import { fileView } from "../lib/chat";
import "./AsciiDocPreview.css";

// GitHub's rules — no script, handler, style or frame, ids prefixed so a
// document cannot shadow the window's — plus the classes asciidoctor marks
// its blocks with, which the look is drawn by.
const schema: Schema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames ?? []), "colgroup", "col"],
  attributes: { ...defaultSchema.attributes, "*": [...(defaultSchema.attributes?.["*"] ?? []), "className"] },
};
const PREFIX = defaultSchema.clobberPrefix ?? "";

const readFile = (path: string) => fileView({ path, side: "worktree" }).then((view) => view.new);

/** An AsciiDoc file of the open folder as it renders, with its includes put in. */
export function AsciiDocPreview({ text, path }: { text: string; path: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    asciidocHtml(text, path, readFile).then(
      (done) => current && (setHtml(done), setError(null)),
      (e) => current && setError(String(e)),
    );
    return () => {
      current = false;
    };
  }, [text, path]);
  const content = useMemo(
    () => html !== null && toJsxRuntime(sanitize(fromParse5(parseFragment(html)), schema), { Fragment, jsx, jsxs }),
    [html],
  );

  // A link out opens in the browser; one within the document scrolls to it.
  const follow = (e: MouseEvent<HTMLDivElement>) => {
    const href = (e.target as Element).closest("a")?.getAttribute("href");
    if (!href) return;
    e.preventDefault();
    if (href.startsWith("#")) document.getElementById(PREFIX + decodeURIComponent(href.slice(1)))?.scrollIntoView();
    else if (/^(https?:|mailto:)/i.test(href)) void openUrl(href);
  };

  if (error) return <div className="file-viewer-note">Could not render: {error}</div>;
  return (
    <div className="adoc" onClick={follow}>
      {content}
    </div>
  );
}
