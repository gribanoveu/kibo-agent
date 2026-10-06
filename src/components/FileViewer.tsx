import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { useFileView } from "../hooks/useFileView";
import { fileRows, paintRows, type Painted } from "../lib/diffRows";
import { highlight, languageOf } from "../lib/highlight";
import { fileWrite, type FileSide, type FileTarget } from "../lib/chat";
import { DiffView } from "./DiffView";
import { Markdown } from "./Markdown";
import { AsciiDocPreview } from "./AsciiDocPreview";
import { sameFile, stepThrough, type Edits } from "../hooks/useOpenFiles";
import { useShortcuts } from "../hooks/useShortcuts";
import { useStaging } from "../hooks/useStaging";
import { matches } from "../lib/shortcuts";
import { onWindowClose } from "../lib/window";
import { Tabs } from "./Tabs";
import "./FileViewer.css";

// CodeMirror is a third of a megabyte, and only the IDE layout edits: loaded
// the first time a file is opened there.
const FileEditor = lazy(() => import("./FileEditor").then((m) => ({ default: m.FileEditor })));

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
  unsaved,
}: {
  files: FileTarget[];
  active: FileTarget;
  preview: FileTarget | null;
  onActivate: (target: FileTarget) => void;
  onPin: (target: FileTarget) => void;
  onClose: (target: FileTarget) => void;
  /** A file with an edit not saved: its tab says so. */
  unsaved: (target: FileTarget) => boolean;
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
              {unsaved(file) && <span className="file-tab-unsaved" title="Not saved" aria-label="Not saved" />}
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
  edits,
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
   * The IDE layout's viewer: a file opens whole rather than as its diff, and
   * can be edited there; the heading is one row — how to show the file sits
   * beside its tab, and the path is the tab's tooltip. Escape does not close
   * the files here: it is a key the editor's user presses for other things.
   */
  compact?: boolean;
  /** The edits not saved yet, by file. A tab with one is saved before it closes. */
  edits?: Edits;
}) {
  const { view, error, reload } = useFileView(target, workspace);
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
  // Beside the chat a file is opened to see what changed in it; in the IDE
  // layout, to be read — as it renders when it is Markdown or AsciiDoc, whole otherwise.
  const [mode, setMode] = useState<Mode>(compact ? "preview" : "diff");
  const text = view && !view.unviewable ? view : null;
  const changed = !!text && text.old !== text.new;
  const language = languageOf(target.path);
  const renders = language === "markdown" || language === "asciidoc";
  // Only the ways this file can be shown: no diff without changes, no preview
  // but for a document — which, unchanged, reads best as it renders.
  const shown: Mode =
    mode === "diff" && !changed ? (renders ? "preview" : "file") : mode === "preview" && !renders ? "file" : mode;
  // Only a file of the open folder is edited — not a side of a diff.
  const draftOf = (file: FileTarget) => (file.side === "worktree" ? edits?.drafts[file.path] : undefined);
  const draft = draftOf(target);
  const onDisk = view && !view.unviewable ? view.new : null;
  const editing = compact && edits && target.side === "worktree" && shown === "file" && view && !view.unviewable;
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // The file changed on disk under the edit — the agent wrote it, or it is
  // gone. Not while a save is landing: the re-read comes before the draft goes.
  const stale = !!draft && !!view && !view.unviewable && draft.base !== onDisk && !saving;
  // The files whose last save did not land, by path, which a closing tab
  // waits on; and the save under way. A save right after one that landed has
  // nothing to write: the re-read told the editor what the disk holds.
  const unsaved = useRef(new Set<string>());
  const inflight = useRef<Promise<unknown> | null>(null);

  /**
   * Writes `file`'s text over `from`, the disk version it was made on — if it
   * says anything that is not there yet. Not over a file that changed on disk
   * under the edit (`conflict`): the banner asks first.
   */
  const persist = (file: FileTarget, text: string, from: string | null, conflict: boolean) => {
    const path = file.path;
    if (from !== null && from.replace(/\r\n/g, "\n") === text) {
      if (!inflight.current) unsaved.current.delete(path);
      return;
    }
    // ponytail: a second save while one is landing waits for the next pause,
    // blur or tab change; queue it if typing that fast turns out to matter.
    if (conflict || inflight.current) {
      unsaved.current.add(path);
      return;
    }
    setSaving(true);
    setSaveError(null);
    const run = (async () => {
      try {
        const saved = (await fileWrite(path, from, text)).kind === "saved";
        if (sameFile(file, target)) await reload();
        if (saved) {
          unsaved.current.delete(path);
          edits?.drop(path);
        } else {
          unsaved.current.add(path);
        }
      } catch (e) {
        unsaved.current.add(path);
        setSaveError(String(e));
      } finally {
        inflight.current = null;
        setSaving(false);
      }
    })();
    inflight.current = run;
  };
  // From the editor: ⌘S, a pause, focus leaving, the tab going. `base` is what
  // the text was made on — saved straight after typing, before any draft, the
  // disk may already hold something newer.
  const save = (text: string, base: string) => {
    if (!edits) return;
    edits.edit(target.path, base, text);
    persist(target, text, draft ? draft.base : base, stale);
  };
  // A tab is saved before it goes; one that cannot be — changed on disk, or
  // the write failed — stays, with the banner saying why. Closed by the window
  // itself (narrowing, another folder), a tab keeps its edit for later.
  const close = (file: FileTarget) => {
    // The editor holds the newest text, and losing focus saves it; the draft,
    // when the editor had none to give, or the tab is not the one showing.
    if (sameFile(file, target)) (document.activeElement as HTMLElement | null)?.blur();
    const pending = draftOf(file);
    if (pending) persist(file, pending.text, pending.base, sameFile(file, target) && stale);
    const done = () => {
      if (!unsaved.current.has(file.path)) onClose(file);
      else if (!sameFile(file, target)) onActivate(file);
    };
    // At once when nothing is being written.
    if (inflight.current) void inflight.current.then(done);
    else done();
  };
  // Not ⌘W: once the last tab is gone, the next press would close the window.
  useShortcuts({ closeFile: () => close(target) });

  // Closing the window saves every edit of the folder first, the one being
  // typed included. One that cannot be saved keeps the window open, once: its
  // tab comes up with the banner saying why, and closing again leaves it.
  const [leaving, setLeaving] = useState(false);
  const beforeClose = useRef(async () => false);
  beforeClose.current = async () => {
    if (!edits || leaving) return false;
    await inflight.current;
    // The editor has the newest text of its file, and losing focus saves it.
    const typing = !!editing && !!document.activeElement?.closest(".file-editor");
    const drafts = Object.entries(edits.drafts);
    if (typing) (document.activeElement as HTMLElement).blur();
    await inflight.current;
    for (const [path, { text, base }] of drafts) {
      if (typing && path === target.path) continue;
      persist({ path, side: "worktree" }, text, base, false);
      await inflight.current;
    }
    const kept = [...drafts.map(([path]) => path), ...(typing ? [target.path] : [])].find((path) => unsaved.current.has(path));
    if (kept === undefined) return false;
    setLeaving(true);
    onActivate({ path: kept, side: "worktree" });
    return true;
  };
  useEffect(() => onWindowClose(() => beforeClose.current()), []);

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
          ...(renders ? [{ id: "preview" as const, label: "Preview" }] : []),
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
        if (e.key === "Escape" && !compact) onCloseAll();
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
          onClose={close}
          unsaved={(file) => !!draftOf(file)}
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
      {(stale || saveError) && (
        <div className="file-viewer-banner" role="alert">
          {leaving && <span className="file-viewer-banner-text">Close the window again to leave this unsaved.</span>}
          {saveError ? (
            <span className="file-viewer-banner-text">Not saved: {saveError}</span>
          ) : (
            <>
              <span className="file-viewer-banner-text">
                {onDisk === null ? "Deleted on disk" : "Changed on disk"} since you began editing.
              </span>
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => {
                  unsaved.current.delete(target.path);
                  edits?.drop(target.path);
                }}
              >
                {onDisk === null ? "Discard mine" : "Load from disk"}
              </button>
              {/* Kept, it is saved over the disk's version at once, as any edit is. */}
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => {
                  edits?.rebase(target.path, onDisk);
                  if (draft) persist(target, draft.text, onDisk, false);
                }}
              >
                Keep mine
              </button>
            </>
          )}
        </div>
      )}
      <div className="file-viewer-body">
        {editing && (onDisk !== null || draft) ? (
          <Suspense fallback={null}>
            <FileEditor
              key={target.path}
              path={target.path}
              text={draft?.text ?? onDisk ?? ""}
              wrap={wrap}
              onChange={(text, base) => edits!.edit(target.path, base, text)}
              onSave={save}
            />
          </Suspense>
        ) : error ? (
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
              {language === "asciidoc" ? (
                <AsciiDocPreview text={draft?.text ?? view.new} path={target.path} />
              ) : (
                <Markdown text={draft?.text ?? view.new} streaming={false} />
              )}
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
