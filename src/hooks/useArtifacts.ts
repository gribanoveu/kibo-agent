import { useState } from "react";
import { useLiveRead } from "./useLiveRead";
import {
  artifactFiles,
  artifactFreeSpace,
  artifactList,
  artifactRename,
  artifactReveal,
  artifactSaveAs,
  onArtifactsChanged,
  onCollectionsChanged,
  type ArtifactFile,
  type ArtifactKind,
  type Collection,
} from "../lib/artifacts";
import { meetingDelete } from "../lib/meeting";

/** Where the page is: the top, a kind, a collection, a folder inside it. */
export type Place = { kind: ArtifactKind | null; id: string | null; folder: string };

const TOP: Place = { kind: null, id: null, folder: "" };

/** A row of the list where the page is; `id` is unique among its neighbours. */
export type Row =
  | { type: "kind"; id: string; kind: ArtifactKind; name: string; count: number }
  | { type: "collection"; id: string; collection: Collection; name: string }
  | { type: "folder"; id: string; name: string; path: string; bytes: number }
  | { type: "file"; id: string; name: string; file: ArtifactFile };

export const KIND_NAMES: Record<ArtifactKind, string> = { meetings: "Meetings", prototypes: "Prototypes" };
const KINDS = Object.keys(KIND_NAMES) as ArtifactKind[];

/**
 * The Artifacts page's state (docs/38-artifacts.md): the collections, read
 * while the page is on screen and again on `onCollectionsChanged` — never on a
 * timer — the open collection's files on `onArtifactsChanged`, and where the page is.
 * The page's own changes read again when they are done; so does a change to
 * `chats`, the Chat mode chats' ids — a prototype goes with its chat, and no
 * backend signal says a chat was deleted.
 */
export function useArtifacts(chats = "", on = true) {
  const [place, setPlace] = useState<Place>(TOP);
  // Moves when the page changed something itself: no signal says a rename.
  const [round, setRound] = useState(0);
  const reload = () => setRound((n) => n + 1);
  const list = useLiveRead(artifactList, onCollectionsChanged, [], on, `${round}#${chats}`);
  // Read with the list: what frees or fills the disk here is what changes the list.
  const free = useLiveRead(artifactFreeSpace, onCollectionsChanged, null, on, `${round}#${chats}`);

  const kind = place.kind;
  const open = kind && place.id ? { kind, id: place.id } : null;
  const at = open ? `${open.kind}/${open.id}` : "";
  // The files carry which collection they are of: until the new one's land, there are none to show.
  const read = useLiveRead(
    () => (open ? artifactFiles(open.kind, open.id).then((files) => ({ at, files })) : Promise.resolve({ at, files: [] })),
    onArtifactsChanged,
    { at: "", files: [] as ArtifactFile[] },
    on && open !== null,
    `${at}#${round}#${chats}`,
  );
  const files = open && read.value.at === at ? read.value.files : null;
  const collection = open ? (list.value.find((c) => c.kind === open.kind && c.id === open.id) ?? null) : null;

  const rows: Row[] =
    kind === null
      ? KINDS.map((k) => ({ type: "kind" as const, id: k, kind: k, name: KIND_NAMES[k], count: list.value.filter((c) => c.kind === k).length })).filter(
          // A kind with nothing in it is not shown — Meetings with its feature off among them.
          (row) => row.count > 0,
        )
      : open === null
        ? list.value.filter((c) => c.kind === kind).map((c) => ({ type: "collection" as const, id: c.id, collection: c, name: c.title }))
        : rowsIn(files ?? [], place.folder);

  return {
    place,
    rows,
    collection,
    /** Every collection's bytes together. */
    used: list.value.reduce((sum, c) => sum + c.bytes, 0),
    /** Free on the disk they are kept on; `null` until read, or when it could not be. */
    free: free.value,
    /** The open collection's files; `null` while they are read. */
    files,
    error: open ? (read.error ?? null) : list.error,
    loaded: open ? files !== null : true,
    openKind: (k: ArtifactKind) => setPlace({ kind: k, id: null, folder: "" }),
    openCollection: (id: string) => kind && setPlace({ kind, id, folder: "" }),
    openFolder: (path: string) => setPlace({ ...place, folder: path }),
    /** One level up; at the top, nothing. */
    up: () => setPlace(upFrom(place)),
    go: setPlace,
    rename: async (c: Collection, title: string) => {
      await artifactRename(c.kind, c.id, title);
      reload();
    },
    /** A meeting deleted; a prototype goes with its chat, through `removeChat`. `false` when the chat must stay. */
    remove: async (c: Collection, removeChat: (id: string) => Promise<boolean>) => {
      if (c.kind === "meetings") await meetingDelete(c.id);
      else if (!(await removeChat(c.id))) return false;
      reload();
      return true;
    },
    reveal: (c: Collection) => artifactReveal(c.kind, c.id),
    /** A file of the open collection saved where the user chooses; where it went, or `null` when they cancelled. */
    saveAs: (file: ArtifactFile, name?: string) => (open ? artifactSaveAs(open.kind, open.id, file.path, name) : Promise.resolve(null)),
  };
}

export function upFrom(place: Place): Place {
  if (place.folder) return { ...place, folder: place.folder.split("/").slice(0, -1).join("/") };
  if (place.id) return { ...place, id: null };
  return TOP;
}

/** The folders and files directly in `folder` of a collection, in the order the backend gave — folders first. */
export function rowsIn(files: ArtifactFile[], folder: string): Row[] {
  const prefix = folder ? `${folder}/` : "";
  const rows: Row[] = [];
  const folders = new Map<string, Extract<Row, { type: "folder" }>>();
  for (const file of files) {
    if (!file.path.startsWith(prefix)) continue;
    const rest = file.path.slice(prefix.length);
    const slash = rest.indexOf("/");
    if (slash < 0) {
      rows.push({ type: "file", id: file.path, name: rest, file });
      continue;
    }
    const name = rest.slice(0, slash);
    const known = folders.get(name);
    if (known) known.bytes += file.bytes;
    else {
      const row = { type: "folder" as const, id: `${prefix}${name}/`, name, path: `${prefix}${name}`, bytes: file.bytes };
      folders.set(name, row);
      rows.push(row);
    }
  }
  return rows;
}
