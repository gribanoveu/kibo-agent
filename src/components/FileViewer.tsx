import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { useFileView } from "../hooks/useFileView";
import { fileRows, paintRows, type Painted } from "../lib/diffRows";
import { highlight, languageOf } from "../lib/highlight";
import type { FileSide, FileTarget } from "../lib/chat";
import { DiffView } from "./DiffView";
import { Markdown } from "./Markdown";
import { sameFile, stepThrough } from "../hooks/useOpenFiles";
import { useStaging } from "../hooks/useStaging";
import { matches } from "../lib/shortcuts";
import { Tabs } from "./Tabs";
import "./FileViewer.css";

const SIDE: Record<FileSide, string | null> = { unstaged: "Unstaged", staged: "Staged", worktree: null };
const SIDE_LETTER: Record<FileSide, string | null> = { unstaged: "U", staged: "S", worktree: null };

type Mode = "diff" | "file" | "preview";

const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

/**
 * The strip of open files: a click shows one, a double click keeps the
 * preview tab (drawn in italics), its cross or a middle click closes it.
 */
function FileTabs({
  files,
  active,
  preview,
  onActivate,
  onPin,
  onClose,
}: {
  files: FileTarget[];
  active: FileTarget;
  preview: FileTarget | null;
  onActivate: (target: FileTarget) => void;
  onPin: (target: FileTarget) => void;
  onClose: (target: FileTarget) => void;
}) {
  const shown = useRef<HTMLDivElement>(null);
  // A block, not an expression: Chromium's scrollIntoView returns a promise,
  // which React would take for the effect's cleanup.
  useEffect(() => {
    shown.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active]);
  return (
    <div className="file-tabs" role="tablist" aria-label="Open files">
      {files.map((file) => {
        const on = sameFile(file, active);
        const passing = !!preview && sameFile(file, preview);
        // The side is said only when the same file is open from both.
        const twin = files.some((f) => f !== file && f.path === file.path);
        return (
          <div
            key={`${file.side}:${file.path}`}
            ref={on ? shown : undefined}
            className={`file-tab${on ? " active" : ""}${passing ? " preview" : ""}`}
            title={passing ? `${file.path} — double-click to keep` : file.path}
            onDoubleClick={() => onPin(file)}
            onAuxClick={(e) => e.button === 1 && onClose(file)}
          >
            <button type="button" role="tab" aria-selected={on} className="file-tab-name" onClick={() => onActivate(file)}>
              {fileName(file.path)}
              {twin && SIDE_LETTER[file.side] && <span className="file-tab-side">{SIDE_LETTER[file.side]}</span>}
            </button>
            <button type="button" className="file-tab-close" title="Close" onClick={() => onClose(file)}>
              <X size={12} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

// ponytail: past this the file stays uncoloured — Shiki's JavaScript engine
// takes seconds on a huge file; colour in chunks or a worker if that matters.
const MAX_COLOURED_CHARS = 300_000;

/** Both versions of a file coloured by its language, once Shiki has them; `null` until then or without one. */
function useColours(old: string | null, next: string | null, path: string) {
  const [colours, setColours] = useState<{ old: Painted | null; next: Painted | null } | null>(null);
  const lang = languageOf(path);
  useEffect(() => {
    setColours(null);
    if (!lang || (old?.length ?? 0) + (next?.length ?? 0) > MAX_COLOURED_CHARS) return;
    let live = true;
    const paint = async (text: string | null): Promise<Painted | null> => {
      const lines = text ? await highlight(text, lang) : null;
      return lines?.map((tokens) => tokens.map((t) => ({ text: t.content, style: t.htmlStyle }))) ?? null;
    };
    void Promise.all([paint(old), paint(next)]).then(([o, n]) => live && setColours({ old: o, next: n }));
    return () => {
      live = false;
    };
  }, [old, next, lang]);
  return colours;
}

/**
 * The column beside the chat that shows the open files, one at a time: what
 * changed in it, the whole of it with the changes in place, or — Markdown —
 * the file as it reads. Opened from Changes and Files, a tab per file.
 */
export function FileViewer({
  files,
  active: target,
  preview,
  workspace,
  onActivate,
  onPin,
  onClose,
  onCloseAll,
  wrap,
  compact = false,
}: {
  files: FileTarget[];
  active: FileTarget;
  /** The tab the next single click reuses, if one is. */
  preview: FileTarget | null;
  workspace: string | null;
  /** Shows a file: its tab if open, else in the preview tab. */
  onActivate: (target: FileTarget) => void;
  onPin: (target: FileTarget) => void;
  onClose: (target: FileTarget) => void;
  onCloseAll: () => void;
  /** Long lines wrap rather than scroll sideways — Settings → Appearance. */
  wrap: boolean;
  /**
   * One heading row, for the IDE layout: how to show the file sits beside
   * its tab, the path is the tab's tooltip, and Escape closes them all.
   */
  compact?: boolean;
}) {
  const { view, error } = useFileView(target, workspace);
  // The changed files, in the order the Changes panel lists them, to step
  // through without going back to it.
  const { unstaged, staged } = useStaging(true, workspace);
  const changes: FileTarget[] = [
    ...unstaged.map((f) => ({ path: f.path, side: "unstaged" as const })),
    ...staged.map((f) => ({ path: f.path, side: "staged" as const })),
  ];
  const at = changes.findIndex((f) => sameFile(f, target));
  const step = (by: 1 | -1) => {
    const next = stepThrough(changes, target, by);
    if (next) onActivate(next);
  };
  const [mode, setMode] = useState<Mode>("diff");
  const text = view && !view.unviewable ? view : null;
  const changed = !!text && text.old !== text.new;
  const markdown = languageOf(target.path) === "markdown";
  // Only the ways this file can be shown: no diff without changes, no preview
  // but for Markdown — which, unchanged, reads best as it renders.
  const shown: Mode =
    mode === "diff" && !changed ? (markdown ? "preview" : "file") : mode === "preview" && !markdown ? "file" : mode;
  // On the texts, not the view: a re-read of an unchanged file makes a new
  // object with the same strings, and redrawing it all would be wasted.
  const oldText = text?.old ?? null;
  const newText = text?.new ?? null;
  const readable = text !== null;
  const plain = useMemo(
    () => (readable ? fileRows(oldText ?? "", newText ?? "", shown !== "diff") : []),
    [readable, oldText, newText, shown],
  );
  const colours = useColours(oldText, newText, target.path);
  const rows = useMemo(() => (colours ? paintRows(plain, colours.old, colours.next) : plain), [plain, colours]);
  const add = rows.filter((row) => row.kind === "add").length;
  const del = rows.filter((row) => row.kind === "del").length;
  const controls = (
    <>
      {SIDE[target.side] && <span className="file-viewer-side">{SIDE[target.side]}</span>}
      <span className="file-viewer-stat">
        {add > 0 && <span className="add">+{add}</span>}
        {del > 0 && <span className="del">-{del}</span>}
      </span>
      <Tabs
        label="Show"
        value={shown}
        onChange={setMode}
        tabs={[
          { id: "diff", label: "Diff", disabled: !changed, title: changed ? undefined : "No changes" },
          { id: "file", label: "File" },
          ...(markdown ? [{ id: "preview" as const, label: "Preview" }] : []),
        ]}
      />
      {changes.length > 0 && (
        <span className="file-viewer-step">
          <button type="button" className="iconbtn" title="Previous changed file (Alt+↑)" onClick={() => step(-1)}>
            <ChevronUp size={14} />
          </button>
          <span className="file-viewer-step-count">
            {at < 0 ? "–" : at + 1} / {changes.length}
          </span>
          <button type="button" className="iconbtn" title="Next changed file (Alt+↓)" onClick={() => step(1)}>
            <ChevronDown size={14} />
          </button>
        </span>
      )}
    </>
  );

  return (
    <section
      className="file-viewer"
      aria-label="File viewer"
      onKeyDown={(e) => {
        if (e.defaultPrevented) return;
        if (e.key === "Escape") onCloseAll();
        else if (matches(e, "nextFile") || matches(e, "prevFile")) {
          e.preventDefault();
          step(matches(e, "nextFile") ? 1 : -1);
        }
      }}
    >
      <div className="file-viewer-tabs">
        <FileTabs
          files={files}
          active={target}
          preview={preview}
          onActivate={onActivate}
          onPin={onPin}
          onClose={onClose}
        />
        {compact ? (
          controls
        ) : (
          <button type="button" className="iconbtn" title="Close all" onClick={onCloseAll}>
            <X size={14} />
          </button>
        )}
      </div>
      {!compact && (
        <div className="file-viewer-head">
          <div className="file-viewer-title" title={target.path}>
            <bdi>{target.path}</bdi>
          </div>
          {controls}
        </div>
      )}
      <div className="file-viewer-body">
        {error ? (
          <div className="file-viewer-note">{error}</div>
        ) : !view ? null : view.unviewable === "binary" ? (
          <div className="file-viewer-note">A binary file — not shown.</div>
        ) : view.unviewable === "tooLarge" ? (
          <div className="file-viewer-note">Too large to show here.</div>
        ) : view.old === null && view.new === null ? (
          <div className="file-viewer-note">Not on disk.</div>
        ) : shown === "preview" ? (
          view.new === null ? (
            <div className="file-viewer-note">Deleted — nothing to preview.</div>
          ) : (
            <div className="file-viewer-preview">
              <Markdown text={view.new} streaming={false} />
            </div>
          )
        ) : rows.length === 0 ? (
          <div className="file-viewer-note">Empty file.</div>
        ) : (
          <DiffView rows={rows} virtual wrap={wrap} />
        )}
      </div>
    </section>
  );
}
