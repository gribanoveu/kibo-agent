import { useState } from "react";
import { AudioLines, ChevronRight, File, FileCode, FileText, Folder, Image, MoreHorizontal } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Markdown } from "./Markdown";
import { Modal } from "./Modal";
import { Page, PageSheet } from "./Page";
import { PrototypeFrame } from "./PrototypeFrame";
import { KIND_NAMES, useArtifacts, type Place, type Row } from "../hooks/useArtifacts";
import { useLiveRead } from "../hooks/useLiveRead";
import { artifactRead, artifactUrl, onArtifactsChanged, type ArtifactFile, type ArtifactKind, type Collection, type FileView } from "../lib/artifacts";
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
  new Date(rfc3339).toLocaleString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });

/**
 * What the app made, as folders (docs/38-artifacts.md): a kind, its
 * collections — a meeting, a prototype — and their files, as a file manager
 * lists them. A click picks a row, a double click opens it, the crumbs
 * go back up; a file picked opens in the panel on the right. A row's
 * menu renames, deletes, shows in Finder, saves a file elsewhere.
 */
export function ArtifactsPage({ openPage, openChat, removeChat, chats }: PageContext) {
  const artifacts = useArtifacts(chats.map((c) => c.id).join());
  const [picked, setPicked] = useState<string | null>(null);
  const [said, setSaid] = useState<{ text: string; error: boolean } | null>(null);
  const [renaming, setRenaming] = useState<{
    collection: Collection;
    title: string;
    error: string | null;
  } | null>(null);
  const [deleting, setDeleting] = useState<Collection | null>(null);
  const { place, collection } = artifacts;

  // What an action did, or why it did not, on the line over the list.
  const act = (work: () => Promise<unknown>) => {
    setSaid(null);
    work().catch((e) => setSaid({ text: String(e), error: true }));
  };
  const rename = () => {
    if (!renaming) return;
    const { collection: c, title } = renaming;
    artifacts.rename(c, title).then(
      () => setRenaming(null),
      (e) => setRenaming({ ...renaming, error: String(e) }),
    );
  };
  const remove = () => {
    if (!deleting) return;
    const c = deleting;
    setDeleting(null);
    act(async () => {
      if (!(await artifacts.remove(c, removeChat)))
        setSaid({
          text: "Stop the running turn in that chat before deleting it",
          error: true,
        });
    });
  };
  const saveAs = (file: ArtifactFile) => {
    const stem = (file.path.split("/").pop() ?? file.path).replace(/\.[^.]*$/, "");
    // A meeting's files are all called the same: the meeting names them. A prototype's page is the prototype.
    const name = !collection
      ? undefined
      : collection.kind === "meetings"
        ? `${collection.title} ${stem}`
        : file.path === collection.page
          ? collection.title
          : undefined;
    act(async () => {
      const dest = await artifacts.saveAs(file, name);
      if (dest)
        setSaid({
          text: `Saved to ${dest.split(/[\\/]/).pop()}`,
          error: false,
        });
    });
  };
  const menu = (row: Row) => {
    if (row.type === "file")
      return [
        {
          value: "save",
          label: "Save as…",
          hint: "A copy where you choose",
          run: () => saveAs(row.file),
        },
      ];
    if (row.type !== "collection") return [];
    const c = row.collection;
    return [
      {
        value: "rename",
        label: "Rename…",
        hint: "",
        run: () => setRenaming({ collection: c, title: c.title, error: null }),
      },
      c.kind === "meetings"
        ? {
            value: "open",
            label: "Open in Meetings",
            hint: "Its notes, transcript and speakers",
            run: () => openPage("meetings", c.id),
          }
        : {
            value: "open",
            label: "Open chat",
            hint: "The chat that made it, with its versions",
            run: () => openChat(c.id),
          },
      {
        value: "reveal",
        label: "Show in Finder",
        hint: "The folder it is kept in",
        run: () => act(() => artifacts.reveal(c)),
      },
      {
        value: "delete",
        label: "Delete…",
        hint: c.kind === "prototypes" ? "With its chat" : "",
        run: () => setDeleting(c),
      },
    ];
  };

  // A new place starts with nothing picked.
  const go = (to: () => void) => {
    to();
    setPicked(null);
  };
  const open = (row: Row) => {
    if (row.type === "kind") go(() => artifacts.openKind(row.kind));
    else if (row.type === "collection") go(() => artifacts.openCollection(row.collection.id));
    else if (row.type === "folder") go(() => artifacts.openFolder(row.path));
    else setPicked(row.id);
  };

  const file = artifacts.rows.find((row): row is Extract<Row, { type: "file" }> => row.type === "file" && row.id === picked)?.file;

  const crumbs: { label: string; to: Place }[] = [{ label: "Artifacts", to: { kind: null, id: null, folder: "" } }];
  if (place.kind)
    crumbs.push({
      label: KIND_NAMES[place.kind],
      to: { kind: place.kind, id: null, folder: "" },
    });
  if (place.kind && place.id)
    crumbs.push({
      label: collection?.title ?? place.id,
      to: { ...place, folder: "" },
    });
  place.folder
    .split("/")
    .filter(Boolean)
    .forEach((name, at, all) =>
      crumbs.push({
        label: name,
        to: { ...place, folder: all.slice(0, at + 1).join("/") },
      }),
    );

  return (
    <Page
      title="Artifacts"
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
          <span className="artifacts-space" aria-label="Disk space">
            {sizeLabel(artifacts.used)} used
            {artifacts.free !== null && ` · ${sizeLabel(artifacts.free)} free on disk`}
          </span>
        </>
      }
      sheet={
        file && place.kind && place.id ? (
          <PageSheet icon={FILE_ICONS[file.view]} title={file.path.split("/").pop() ?? file.path} label={`File ${file.path}`} onClose={() => setPicked(null)}>
            <ArtifactViewer kind={place.kind} id={place.id} file={file} />
          </PageSheet>
        ) : undefined
      }
    >
      <div className="artifacts-body">
        {said && <p className={said.error ? "artifacts-error" : "artifacts-said"}>{said.text}</p>}
        {artifacts.error && <p className="artifacts-error">{artifacts.error}</p>}
        {!artifacts.error && artifacts.loaded && artifacts.rows.length === 0 && (
          <p className="artifacts-empty">
            {place.kind === null ? "Nothing here yet. Meetings made into notes and prototypes presented in a chat show up here." : "This folder is empty."}
          </p>
        )}
        {artifacts.rows.length > 0 && (
          <div className="artifacts-list" role="listbox" aria-label="Contents">
            {artifacts.rows.map((row) => (
              <ArtifactRow
                key={row.id}
                row={row}
                picked={row.id === picked}
                onPick={() => (row.type === "file" ? open(row) : setPicked(row.id))}
                onOpen={() => open(row)}
                actions={menu(row)}
              />
            ))}
          </div>
        )}
      </div>
      <Modal
        title={`Rename “${renaming?.collection.title ?? ""}”`}
        open={renaming !== null}
        onClose={() => setRenaming(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setRenaming(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" disabled={!renaming?.title.trim()} onClick={rename}>
              Rename
            </button>
          </>
        }
      >
        <input
          className="artifacts-rename"
          autoFocus
          value={renaming?.title ?? ""}
          aria-label="New name"
          onChange={(e) => renaming && setRenaming({ ...renaming, title: e.target.value, error: null })}
          // A form field's Enter, as in every settings form here — not a shortcut.
          onKeyDown={(e) => {
            if (!matches(e, "accept")) return;
            e.preventDefault();
            rename();
          }}
        />
        {renaming?.error && <p className="artifacts-error">{renaming.error}</p>}
      </Modal>
      <Modal
        title={`Delete “${deleting?.title ?? ""}”?`}
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDeleting(null)}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" onClick={remove}>
              Delete
            </button>
          </>
        }
      >
        <p className="artifacts-confirm">
          {deleting?.kind === "prototypes"
            ? `Deletes the chat “${deleting.title}” with its prototype — every version of it. A prototype lives as long as its chat.`
            : "Deletes the meeting with its recording, notes and documents; what the voice memory heard in it is forgotten."}
        </p>
      </Modal>
    </Page>
  );
}

type Action = { value: string; label: string; hint: string; run: () => void };

function ArtifactRow({ row, picked, onPick, onOpen, actions }: { row: Row; picked: boolean; onPick: () => void; onOpen: () => void; actions: Action[] }) {
  const Icon = row.type === "file" ? FILE_ICONS[row.file.view] : Folder;
  const detail = row.type === "kind" ? `${row.count}` : row.type === "collection" ? when(row.collection.updated) : "";
  const bytes = row.type === "collection" ? row.collection.bytes : row.type === "folder" ? row.bytes : row.type === "file" ? row.file.bytes : 0;
  return (
    <div role="presentation" className={`artifacts-row${picked ? " on" : ""}`}>
      <button
        type="button"
        role="option"
        data-sheet-opener
        aria-selected={picked}
        className="artifacts-row-main"
        onClick={onPick}
        onDoubleClick={onOpen}
      >
        <Icon size={15} className={row.type === "file" ? "artifacts-icon" : "artifacts-icon folder"} />
        <span className="artifacts-name">{row.name}</span>
        <span className="artifacts-detail">{detail}</span>
        <span className="artifacts-size">{bytes > 0 ? sizeLabel(bytes) : ""}</span>
      </button>
      <span className="artifacts-menu">
        {actions.length > 0 && (
          <Dropdown
            label={<MoreHorizontal size={14} aria-label={`Actions for ${row.name}`} />}
            title={`Actions for ${row.name}`}
            options={actions.map(({ value, label, hint }) => ({
              value,
              label,
              hint: hint || undefined,
            }))}
            value=""
            onPick={(value) => actions.find((a) => a.value === value)?.run()}
            below
            right
          />
        )}
      </span>
    </div>
  );
}

/** A file as its `view` says: text read and drawn, the rest loaded from its protocol. */
export function ArtifactViewer({ kind, id, file }: { kind: ArtifactKind; id: string; file: ArtifactFile }) {
  const text = file.view === "markdown" || file.view === "code";
  const at = `${kind}/${id}/${file.path}`;
  const read = useLiveRead(() => artifactRead(kind, id, file.path).then((body) => ({ at, body })), onArtifactsChanged, { at: "", body: "" }, text, at);
  // What loads from its protocol is drawn again on the same signal: the
  // protocol answers `no-store`, so a new element fetches the file as it is now.
  const drawn = useLiveRead(() => Promise.resolve(Date.now()), onArtifactsChanged, 0, !text, at);
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
  if (file.view === "html" && url) return <PrototypeFrame key={drawn.value} className="artifacts-frame" title={`Page ${name}`} src={url} />;
  if (file.view === "image" && url) return <img key={drawn.value} className="artifacts-image" src={url} alt={name} />;
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
