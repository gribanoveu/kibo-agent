import { cloneElement, isValidElement, memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, Code2, Copy, SquareTerminal, Workflow } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { defaultRemarkPlugins, Streamdown, useIsCodeFenceIncomplete, type Components } from "streamdown";
import { highlight, splitLines, type Token } from "../lib/highlight";
import { renderMermaid } from "../lib/mermaid";
import { DiagramView } from "./DiagramView";
import { wrapAsciiTrees } from "../lib/wrapAsciiTrees";
import "streamdown/styles.css";
import "./Markdown.css";

/** A fenced block's text, back out of the React children Streamdown made of it. */
function textOf(node: ReactNode): string {
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return "";
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const label = copied ? "Copied" : "Copy code";
  return (
    <button
      type="button"
      className={`md-code-action${copied ? " copied" : ""}`}
      title={label}
      aria-label={label}
      onClick={() => {
        // A clipboard that refuses loses nothing: the code is on screen.
        navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => setCopied(false), 1500);
        }, () => {});
      }}
    >
      {copied ? <Check size={13} aria-hidden /> : <Copy size={13} aria-hidden />}
    </button>
  );
}

/** Light or dark, as `useTheme` last wrote it to <html>: a diagram is drawn again when it changes. */
function useScheme(): string | undefined {
  const [scheme, setScheme] = useState(() => document.documentElement.dataset.scheme);
  useEffect(() => {
    const root = document.documentElement;
    const watch = new MutationObserver(() => setScheme(root.dataset.scheme));
    watch.observe(root, { attributes: true, attributeFilter: ["data-scheme"] });
    return () => watch.disconnect();
  }, []);
  return scheme;
}

/** `source` drawn as a diagram once it is whole; a `null` source draws nothing. */
function useDiagram(source: string | null): { svg: string | null; error: string | null } {
  const scheme = useScheme();
  const [drawn, setDrawn] = useState<{ svg: string | null; error: string | null }>({ svg: null, error: null });
  useEffect(() => {
    if (source === null) return;
    let live = true;
    renderMermaid(source, scheme === "dark").then(
      (svg) => live && setDrawn({ svg, error: null }),
      (e: unknown) => live && setDrawn({ svg: null, error: e instanceof Error ? e.message : String(e) }),
    );
    return () => {
      live = false;
    };
  }, [source, scheme]);
  return source === null ? { svg: null, error: null } : drawn;
}

/** Fences whose text a shell runs as it stands — not `console`, which carries prompts and output. */
const SHELLS = new Set(["bash", "sh", "zsh", "shell"]);

/** A fenced block: numbered lines, colours once the fence is closed — so a
 * streaming block stays plain and synchronous instead of re-highlighting on
 * every delta. `block` is what tells it from inline code: a fence without a
 * language tag has no class, and is a block all the same. A shell block
 * offers `onPaste` — pasted, not run — once its fence is closed — half a command is not one. */
function Code({
  className,
  block,
  onPaste,
  children,
}: {
  className?: string;
  block: boolean;
  onPaste?: (command: string) => void;
  children?: ReactNode;
}) {
  const lang = block ? (/language-(\S+)/.exec(className ?? "")?.[1]?.toLowerCase() ?? "text") : null;
  const source = lang === null ? "" : textOf(children);
  const incomplete = useIsCodeFenceIncomplete();
  const [tokens, setTokens] = useState<Token[][] | null>(null);
  // A diagram is drawn once its fence is closed: half of one does not parse.
  const diagram = useDiagram(lang === "mermaid" && !incomplete ? source : null);
  const [showCode, setShowCode] = useState(false);
  const [large, setLarge] = useState(false);
  const drawing = diagram.svg !== null && !showCode;
  const image = diagram.svg === null ? "" : `data:image/svg+xml;charset=utf-8,${encodeURIComponent(diagram.svg)}`;

  useEffect(() => {
    setTokens(null);
    if (lang === null || incomplete) return;
    let live = true;
    void highlight(source, lang).then((result) => live && setTokens(result));
    return () => {
      live = false;
    };
  }, [source, lang, incomplete]);

  if (lang === null) return <code className="md-code-inline">{children}</code>;

  return (
    <div className="md-code" data-lang={lang}>
      <div className="md-code-actions">
        {diagram.svg !== null && (
          <button
            type="button"
            className="md-code-action"
            title={showCode ? "Show diagram" : "Show code"}
            aria-label={showCode ? "Show diagram" : "Show code"}
            onClick={() => setShowCode((v) => !v)}
          >
            {showCode ? <Workflow size={13} aria-hidden /> : <Code2 size={13} aria-hidden />}
          </button>
        )}
        {onPaste && SHELLS.has(lang) && !incomplete && (
          <button type="button" className="md-code-action" title="Paste into terminal" aria-label="Paste into terminal" onClick={() => onPaste(source.replace(/\n$/, ""))}>
            <SquareTerminal size={13} aria-hidden />
          </button>
        )}
        <CopyButton text={source} />
      </div>
      {drawing ? (
        // Shrunk to the thread's width; a click opens it the whole window over.
        <button type="button" className="md-diagram" title="Open larger" onClick={() => setLarge(true)}>
          <img src={image} alt="Diagram" />
        </button>
      ) : (
      <div className="md-code-scroll">
        <div className="md-code-body">
        {splitLines(source).map((line, index) => (
          <div key={index} className="md-code-line">
            <span className="md-code-gutter" aria-hidden>
              {index + 1}
            </span>
            <code className="md-code-text">
              {tokens?.[index]?.length
                ? tokens[index].map((token, i) => (
                    <span key={i} style={token.htmlStyle}>
                      {token.content}
                    </span>
                  ))
                : line || " "}
            </code>
          </div>
        ))}
        </div>
      </div>
      )}
      {/* Out of the message, whose styles would reach into it. */}
      {large && image && createPortal(<DiagramView src={image} themed onClose={() => setLarge(false)} />, document.body)}
      {diagram.error !== null && <div className="md-diagram-error">Not drawn as a diagram: {diagram.error}</div>}
    </div>
  );
}

// Streamdown's own elements lean on Tailwind classes, and this app has no
// Tailwind: every element it draws gets a plain class styled in Markdown.css.
const components: Components = {
  h1: ({ children }) => <h1 className="md-h1">{children}</h1>,
  h2: ({ children }) => <h2 className="md-h2">{children}</h2>,
  h3: ({ children }) => <h3 className="md-h3">{children}</h3>,
  h4: ({ children }) => <h4 className="md-h4">{children}</h4>,
  h5: ({ children }) => <h5 className="md-h4">{children}</h5>,
  h6: ({ children }) => <h6 className="md-h4">{children}</h6>,
  p: ({ children }) => <p className="md-p">{children}</p>,
  strong: ({ children }) => <strong className="md-strong">{children}</strong>,
  em: ({ children }) => <em>{children}</em>,
  ul: ({ children }) => <ul className="md-ul">{children}</ul>,
  ol: ({ children }) => <ol className="md-ol">{children}</ol>,
  // A GFM task (`- [ ]`) is the one item that comes with a class: it gets
  // no bullet, and its box is drawn here rather than by the platform.
  li: ({ className, children }) => (
    <li className={className === "task-list-item" ? "md-li md-task" : "md-li"}>{children}</li>
  ),
  input: ({ checked }) => (
    <span className="md-check" role="checkbox" aria-checked={!!checked} aria-readonly>
      {checked && <Check size={10} strokeWidth={3} aria-hidden />}
    </span>
  ),
  blockquote: ({ children }) => <blockquote className="md-quote">{children}</blockquote>,
  table: ({ children }) => (
    <div className="md-table-scroll">
      <table className="md-table">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead>{children}</thead>,
  tbody: ({ children }) => <tbody>{children}</tbody>,
  tr: ({ children }) => <tr>{children}</tr>,
  th: ({ children }) => <th className="md-th">{children}</th>,
  td: ({ children }) => <td className="md-td">{children}</td>,
  hr: () => <hr className="md-hr" />,
  sup: ({ children }) => <sup>{children}</sup>,
  sub: ({ children }) => <sub>{children}</sub>,
  // The CSP lets no outside image load; alt text is what is left of one.
  img: ({ src, alt }) => <img className="md-img" src={typeof src === "string" ? src : undefined} alt={alt} />,
  // A `<code>` under `<pre>` is a fenced block; Streamdown marks it the same way.
  pre: ({ children }) => (isValidElement(children) ? cloneElement(children, { "data-block": "true" } as object) : children),
};

/** A link to a file in the open folder, carried as a fragment: Streamdown's
 * link hardening drops an href it cannot read as a URL — a bare `docs/x.md`
 * is not one — and a fragment is the one relative form it keeps as written. */
const FILE_HREF = "#file:";
const EXTERNAL = /^(https?:|mailto:|tel:|#)/i;

type MdNode = { type: string; url?: string; children?: MdNode[] };
function fileLinks() {
  const walk = (node: MdNode) => {
    if ((node.type === "link" || node.type === "definition") && node.url && !EXTERNAL.test(node.url)) {
      node.url = FILE_HREF + encodeURIComponent(node.url);
    }
    node.children?.forEach(walk);
  };
  return walk;
}
const remarkPlugins = [...Object.values(defaultRemarkPlugins), fileLinks];

/**
 * The model's answer as Markdown.
 *
 * `streaming` turns on Streamdown's repair of half-written markup, and only
 * while the text is still arriving: the repair bets the next delta finishes
 * the markup and drops text to make the bet — everything after an unmatched
 * `![`, the bracket of an unmatched `[`. Mid-stream that costs a frame; on a
 * finished answer it would lose what the model wrote (`arr[0` is ordinary).
 *
 * `animated`, also only while streaming: each new word fades in, cascaded
 * across a delta, so text arriving in bursts reads as a flow rather than
 * jumps. A finished answer renders without the per-word spans.
 *
 * `memo`: every delta re-renders the transcript, and past answers must not
 * re-parse and re-highlight for a token that changed only the last one.
 */
export const Markdown = memo(function Markdown({
  text,
  streaming,
  onPaste,
  onOpenFile,
}: {
  text: string;
  streaming: boolean;
  /** A shell block's terminal button hands its command here; without it there is no button. */
  onPaste?: (command: string) => void;
  /** A link to a file hands its target here, as written; without it the link is plain text. */
  onOpenFile?: (link: string) => void;
}) {
  const bound = useMemo<Components>(
    () => ({
      ...components,
      // A web link opens in the system browser, never in this window. The
      // address is the tooltip: a link the model wrote can carry anything in
      // its query string, and the user should see where a click sends it.
      a: ({ href, children }) => {
        const file = href?.startsWith(FILE_HREF) ? decodeURIComponent(href.slice(FILE_HREF.length)) : null;
        if (file !== null && !onOpenFile) return <span>{children}</span>;
        return (
          <a
            href={href}
            title={file ?? href}
            className="md-link"
            onClick={(event) => {
              event.preventDefault();
              if (file !== null) onOpenFile?.(file);
              else if (href) void openUrl(href);
            }}
          >
            {children}
          </a>
        );
      },
      code: ({ className, children, ...rest }) => (
        <Code className={className} block={"data-block" in rest} onPaste={onPaste}>
          {children}
        </Code>
      ),
    }),
    [onPaste, onOpenFile],
  );
  return (
    <Streamdown
      className="md"
      animated
      isAnimating={streaming}
      parseIncompleteMarkdown={streaming}
      linkSafety={{ enabled: false }}
      remarkPlugins={remarkPlugins}
      components={bound}
    >
      {wrapAsciiTrees(text)}
    </Streamdown>
  );
});
