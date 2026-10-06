import { useEffect, useMemo, useState, type ComponentProps, type MouseEvent } from "react";
import type { Element as HastElement, Nodes } from "hast";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import { parseFragment } from "parse5";
import { fromParse5 } from "hast-util-from-parse5";
import { defaultSchema, sanitize, type Schema } from "hast-util-sanitize";
import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import { openUrl } from "@tauri-apps/plugin-opener";
import { asciidocHtml } from "../lib/asciidoc";
import { fileView } from "../lib/chat";
import { plantumlSvg } from "../lib/plantuml";
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

const text = (node: Nodes): string =>
  node.type === "text" ? node.value : "children" in node ? node.children.map(text).join("") : "";

/**
 * A PlantUML diagram as the engine draws it: an image, so its SVG runs
 * nothing and styles nothing outside it. Its source until then, or with what
 * the engine said when it could not.
 */
function Diagram({ source }: { source: string }) {
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    plantumlSvg(source).then(
      (done) => current && setSvg(done),
      (e) => current && setError(String(e)),
    );
    return () => {
      current = false;
    };
  }, [source]);
  if (svg) return <img className="adoc-diagram" alt="PlantUML diagram" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`} />;
  return (
    <div className="listingblock">
      {error && <div className="title">Could not draw the diagram: {error}</div>}
      <pre>{source}</pre>
    </div>
  );
}

/** A PlantUML file as it draws. */
export const PlantumlPreview = ({ text }: { text: string }) => (
  <div className="adoc">
    <Diagram source={text.replace(/\n+$/, "")} />
  </div>
);

// What `asciidoc.ts` marked as a diagram is drawn; any other block as it is.
const components = {
  div: ({ node, ...props }: ComponentProps<"div"> & { node?: HastElement }) =>
    node && [node.properties.className].flat().includes("plantuml") ? <Diagram source={text(node).replace(/^\n+|\n+$/g, "")} /> : <div {...props} />,
};

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
    () => html !== null && toJsxRuntime(sanitize(fromParse5(parseFragment(html)), schema), { Fragment, jsx, jsxs, components, passNode: true }),
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
