import { useEffect, useState, type CSSProperties } from "react";
import { ChevronRight, File, Folder, FolderOpen } from "lucide-react";
import type { FileStatus, FileTarget, TreeEntry } from "../lib/chat";
import type { Block } from "../lib/chatTurnReducer";
import { touchedFiles, type Touch, type TouchedFile } from "../lib/touchedFiles";
import { useFileTree } from "../hooks/useFileTree";
import { Tabs } from "./Tabs";
import "./FilesPanel.css";

type Props = {
  /** On screen: the folder is read only while it is. */
  active: boolean;
  /** The open chat's transcript: what the agent did is read from its calls. */
  blocks: Block[];
  workspace: string | null;
  /** The file the viewer shows: its row is marked. */
  openFile: FileTarget | null;
  onOpenFile: (target: FileTarget, pin?: boolean) => void;
  /** The IDE layout, where the tree is what the panel is for: it turns to it. */
  ide?: boolean;
};

/** As a file explorer marks them, with the word in the tooltip. */
const STATUS: Record<FileStatus, { letter: string; title: string }> = {
  modified: { letter: "M", title: "Modified" },
  added: { letter: "A", title: "Added to the index" },
  untracked: { letter: "U", title: "Untracked" },
  conflicted: { letter: "!", title: "Conflicted" },
};

type Tree = ReturnType<typeof useFileTree> & { open: (path: string, pin: boolean) => void; shown: string | null };

/**
 * An unfolded folder and the run of folders under it that each hold only the
 * next: drawn as one row, `src / main / java`, as an editor's explorer does.
 * The run ends at a folder that is folded, not read yet, or holds a file or a
 * choice of folders.
 */
function compacted(head: TreeEntry, tree: Tree): TreeEntry[] {
  const run = [head];
  for (let last = head; tree.expanded.has(last.path); ) {
    const listing = tree.listings[last.path];
    const only = listing && listing.entries.length === 1 && listing.more === 0 ? listing.entries[0] : null;
    if (!only?.isDir || !tree.expanded.has(only.path)) break;
    run.push(only);
    last = only;
  }
  return run;
}

/** One folder's entries, and under each unfolded one its own — as deep as unfolded. */
function TreeLevel({ dir, depth, tree }: { dir: string; depth: number; tree: Tree }) {
  const listing = tree.listings[dir];
  const indent = { "--depth": depth } as CSSProperties;
  if (!listing) {
    return (
      <div className="tree-note" style={indent}>
        Loading…
      </div>
    );
  }
  return (
    <>
      {listing.entries.map((entry) =>
        entry.isDir ? (
          <FolderRow key={entry.path} entry={entry} depth={depth} tree={tree} />
        ) : (
          <button
            type="button"
            key={entry.path}
            className={`tree-row${entry.status ? ` ${entry.status}` : ""}${tree.shown === entry.path ? " open" : ""}`}
            style={indent}
            title={entry.path}
            aria-current={tree.shown === entry.path || undefined}
            onClick={() => tree.open(entry.path, false)}
            onDoubleClick={() => tree.open(entry.path, true)}
          >
            <span className="tree-chev" />
            <File className="tree-icon" size={13} aria-hidden />
            <span className="tree-name">{entry.name}</span>
            {entry.status && (
              <span className={`tree-status ${entry.status}`} title={STATUS[entry.status].title}>
                {STATUS[entry.status].letter}
              </span>
            )}
          </button>
        ),
      )}
      {listing.more > 0 && (
        <div className="tree-note" style={indent}>
          …and {listing.more} more
        </div>
      )}
    </>
  );
}

/** A folder, or a run of them in one row; a click folds or unfolds the whole run. */
function FolderRow({ entry, depth, tree }: { entry: TreeEntry; depth: number; tree: Tree }) {
  const open = tree.expanded.has(entry.path);
  const run = compacted(entry, tree);
  const last = run[run.length - 1];
  const Icon = open ? FolderOpen : Folder;
  return (
    <div>
      <button
        type="button"
        className={`tree-row dir${open ? " expanded" : ""}`}
        style={{ "--depth": depth } as CSSProperties}
        aria-expanded={open}
        title={last.path}
        onClick={() => tree.toggle(entry.path)}
      >
        <span className="tree-chev">
          <ChevronRight size={12} />
        </span>
        <Icon className="tree-icon" size={13} aria-hidden />
        <span className="tree-name">
          {run.map((folder, at) => (
            <span key={folder.path} className={at < run.length - 1 ? "tree-part" : undefined}>
              {at > 0 && <span className="tree-sep"> / </span>}
              {folder.name}
            </span>
          ))}
        </span>
        {entry.changed && <span className="tree-changed" title="Changes inside" />}
      </button>
      {open && <TreeLevel dir={last.path} depth={depth + 1} tree={tree} />}
    </div>
  );
}

const TOUCH_LABEL: Record<Touch, string> = {
  read: "read",
  edited: "edited",
  written: "written",
  deleted: "deleted",
  moved: "moved",
};

/** The Files tab: the files the agent worked with in this chat, or the open folder as a tree. */
export function FilesPanel({ active, blocks, workspace, openFile, onOpenFile, ide = false }: Props) {
  const shown = openFile?.side === "worktree" ? openFile.path : null;
  const [view, setView] = useState<"chat" | "folder">(ide ? "folder" : "chat");
  useEffect(() => {
    if (ide) setView("folder");
  }, [ide]);
  const files = touchedFiles(blocks, workspace);
  // The tree is read only while its own tab is the one showing.
  const tree = {
    ...useFileTree(active && view === "folder", workspace),
    open: (path: string, pin: boolean) => onOpenFile({ path, side: "worktree" }, pin),
    shown,
  };
  return (
    <div className="panel-section">
      <Tabs
        label="Files"
        value={view}
        onChange={setView}
        tabs={[
          { id: "chat", label: "In this chat", count: files.length },
          { id: "folder", label: "All files", title: workspace ?? undefined, disabled: !workspace },
        ]}
      />

      <div role="tabpanel">
        {view === "chat" ? (
          files.length === 0 ? (
            <div className="files-empty">Files the agent reads or changes in this chat show up here.</div>
          ) : (
            files.map((file) => (
              <TouchedRow
                key={file.path}
                file={file}
                open={shown === file.path}
                onOpen={(pin) => onOpenFile({ path: file.path, side: "worktree" }, pin)}
              />
            ))
          )
        ) : tree.error ? (
          <div className="files-empty">{tree.error}</div>
        ) : (
          <TreeLevel dir="" depth={0} tree={tree} />
        )}
      </div>
    </div>
  );
}

function TouchedRow({ file, open, onOpen }: { file: TouchedFile; open: boolean; onOpen: (pin: boolean) => void }) {
  const cut = file.path.lastIndexOf("/");
  const dir = cut > 0 ? file.path.slice(0, cut) : null;
  const gone = file.touches[file.touches.length - 1] === "deleted";
  return (
    <button
      type="button"
      className={`files-row${gone ? " gone" : ""}${open ? " open" : ""}`}
      title={file.path}
      aria-current={open || undefined}
      onClick={() => onOpen(false)}
      onDoubleClick={() => onOpen(true)}
    >
      <span className="files-label">
        <span className="files-name">{file.path.slice(cut + 1)}</span>
        {dir && (
          <span className="files-dir">
            <bdi>{dir}</bdi>
          </span>
        )}
      </span>
      <span className="files-touches">
        {file.touches.map((touch) => (
          <span key={touch} className={`files-touch ${touch}`}>
            {TOUCH_LABEL[touch]}
          </span>
        ))}
      </span>
      {(file.add > 0 || file.del > 0) && (
        <span className="files-stat">
          {file.add > 0 && <span className="add">+{file.add}</span>}
          {file.del > 0 && <span className="del">−{file.del}</span>}
        </span>
      )}
    </button>
  );
}
