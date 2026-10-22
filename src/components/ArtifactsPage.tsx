import { useState, type KeyboardEvent } from "react";
import { AudioLines, ChevronRight, File, FileCode, FileText, Folder, Image, Search } from "lucide-react";
import { Markdown } from "./Markdown";
import { Page, PagePanel } from "./Page";
import { PrototypeFrame } from "./PrototypeFrame";
import { KIND_NAMES, useArtifacts, type Place, type Row } from "../hooks/useArtifacts";
import { useLiveRead } from "../hooks/useLiveRead";
import { artifactRead, artifactUrl, onArtifactsChanged, type ArtifactFile, type ArtifactKind, type FileView } from "../lib/artifacts";
import { sizeLabel } from "../lib/prototype";
import { matches } from "../lib/shortcuts";
import type { PageContext } from "./pages";
import "./ArtifactsPage.css";

const FILE_ICONS: Record<FileView, typeof File> = {
  markdown: FileText,
  code: FileCode,
  html: FileCode,
  image: Image,
  audio: AudioLines,
  other: File,
};

const when = (rfc3339: string) =>
  new Date(rfc3339).toLocaleString(undefined, { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" });

/**
 * What the app made, as folders (docs/38-artifacts.md): a kind, its
 * collections — a meeting, a prototype — and their files, as a file manager
 * lists them. A click picks a row, a double click or Enter opens it,
 * Backspace goes up; a file picked opens in the panel on the right.
 */
export function ArtifactsPage({ sideHandle }: PageContext) {
  const artifacts = useArtifacts();
  const [picked, setPicked] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const { place, collection } = artifacts;

  // A new place starts with nothing picked and no filter.
  const go = (to: () => void) => {
    to();
    setPicked(null);
    setFilter("");
  };
  const open = (row: Row) => {
    if (row.type === "kind") go(() => artifacts.openKind(row.kind));
    else if (row.type === "collection") go(() => artifacts.openCollection(row.collection.id));
    else if (row.type === "folder") go(() => artifacts.openFolder(row.path));
    else setPicked(row.id);
  };

  const shown = filter.trim() ? artifacts.rows.filter((row) => row.name.toLowerCase().includes(filter.trim().toLowerCase())) : artifacts.rows;
  const file = artifacts.rows.find((row): row is Extract<Row, { type: "file" }> => row.type === "file" && row.id === picked)?.file;

  const crumbs: { label: string; to: Place }[] = [{ label: "Artifacts", to: { kind: null, id: null, folder: "" } }];
  if (place.kind) crumbs.push({ label: KIND_NAMES[place.kind], to: { kind: place.kind, id: null, folder: "" } });
  if (place.kind && place.id) crumbs.push({ label: collection?.title ?? place.id, to: { ...place, folder: "" } });
  place.folder
    .split("/")
    .filter(Boolean)
    .forEach((name, at, all) => crumbs.push({ label: name, to: { ...place, folder: all.slice(0, at + 1).join("/") } }));

  const keys = (e: KeyboardEvent) => {
    if (e.target instanceof HTMLInputElement || !matches(e, "artifactUp") || crumbs.length < 2) return;
    e.preventDefault();
    go(artifacts.up);
  };

  return (
    <Page
      title="Artifacts"
      sideHandle={sideHandle}
      bar={
        <>
          <nav className="artifacts-crumbs" aria-label="Where you are">
            {crumbs.map((crumb, at) => (
              <span key={at} className="artifacts-crumb">
                {at > 0 && <ChevronRight size={13} className="artifacts-crumb-sep" />}
                <button type="button" aria-current={at === crumbs.length - 1 ? "page" : undefined} onClick={() => go(() => artifacts.go(crumb.to))}>
                  {crumb.label}
                </button>
              </span>
            ))}
          </nav>
          <label className="artifacts-filter">
            <Search size={14} />
            <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Filter by name" aria-label="Filter by name" />
          </label>
        </>
      }
      panel={
        file && place.kind && place.id ? (
          <PagePanel icon={FILE_ICONS[file.view]} title={file.path.split("/").pop() ?? file.path} label={`File ${file.path}`} onClose={() => setPicked(null)}>
            <ArtifactViewer kind={place.kind} id={place.id} file={file} />
          </PagePanel>
        ) : undefined
      }
    >
      {artifacts.error && <p className="artifacts-error">{artifacts.error}</p>}
      {!artifacts.error && artifacts.loaded && shown.length === 0 && (
        <p className="artifacts-empty">
          {filter.trim()
            ? `Nothing here is called “${filter.trim()}”.`
            : place.kind === null
              ? "Nothing here yet. Meetings made into notes and prototypes presented in a chat show up here."
              : "This folder is empty."}
        </p>
      )}
      {shown.length > 0 && (
        <div className="artifacts-list" role="listbox" aria-label="Contents" onKeyDown={keys}>
          {shown.map((row) => (
            <ArtifactRow
              key={row.id}
              row={row}
              picked={row.id === picked}
              onPick={() => (row.type === "file" ? open(row) : setPicked(row.id))}
              onOpen={() => open(row)}
            />
          ))}
        </div>
      )}
    </Page>
  );
}

function ArtifactRow({ row, picked, onPick, onOpen }: { row: Row; picked: boolean; onPick: () => void; onOpen: () => void }) {
  const Icon = row.type === "file" ? FILE_ICONS[row.file.view] : Folder;
  const detail =
    row.type === "kind"
      ? `${row.count}`
      : row.type === "collection"
        ? when(row.collection.updated)
        : "";
  const bytes = row.type === "collection" ? row.collection.bytes : row.type === "folder" ? row.bytes : row.type === "file" ? row.file.bytes : 0;
  return (
    <button
      type="button"
      role="option"
      aria-selected={picked}
      className={`artifacts-row${picked ? " on" : ""}`}
      onClick={onPick}
      onDoubleClick={onOpen}
      onKeyDown={(e) => {
        if (!matches(e, "artifactOpen")) return;
        e.preventDefault();
        onOpen();
      }}
    >
      <Icon size={15} className={row.type === "file" ? "artifacts-icon" : "artifacts-icon folder"} />
      <span className="artifacts-name">{row.name}</span>
      <span className="artifacts-detail">{detail}</span>
      <span className="artifacts-size">{bytes > 0 ? sizeLabel(bytes) : ""}</span>
    </button>
  );
}

/** A file as its `view` says: text read and drawn, the rest loaded from its protocol. */
export function ArtifactViewer({ kind, id, file }: { kind: ArtifactKind; id: string; file: ArtifactFile }) {
  const text = file.view === "markdown" || file.view === "code";
  const at = `${kind}/${id}/${file.path}`;
  const read = useLiveRead(
    () => artifactRead(kind, id, file.path).then((body) => ({ at, body })),
    onArtifactsChanged,
    { at: "", body: "" },
    text,
    at,
  );
  const url = artifactUrl(kind, id, file.path);
  const name = file.path.split("/").pop() ?? file.path;

  if (text) {
    if (read.error) return <p className="artifacts-viewer-note error">{read.error}</p>;
    if (read.value.at !== at) return null;
    const body = file.view === "markdown" ? read.value.body : fenced(read.value.body, name);
    return (
      <div className="artifacts-text">
        <Markdown text={body} streaming={false} />
      </div>
    );
  }
  if (file.view === "html" && url) return <PrototypeFrame className="artifacts-frame" title={`Page ${name}`} src={url} />;
  if (file.view === "image" && url) return <img className="artifacts-image" src={url} alt={name} />;
  if (file.view === "audio" && url) return <audio className="artifacts-audio" controls src={url} aria-label={`Recording ${name}`} />;
  return (
    <p className="artifacts-viewer-note">
      {name} · {sizeLabel(file.bytes)} — no preview for this kind of file.
    </p>
  );
}

/** `text` as one code block of its file's language, in a fence longer than any run of backticks in it. */
export function fenced(text: string, name: string): string {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  const dot = name.lastIndexOf(".");
  return `${fence}${dot > 0 ? name.slice(dot + 1) : ""}\n${text}\n${fence}`;
}
