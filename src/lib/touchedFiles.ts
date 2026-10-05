import type { Block } from "./chatTurnReducer";

// The files the agent worked with in this chat, as the Files tab lists them:
// read from the transcript's own tool calls, so the list is exactly what the
// model did — nothing the backend has to remember.

export type Touch = "read" | "edited" | "written" | "deleted" | "moved";

export type TouchedFile = {
  /** Relative to the open folder, as the tools report it. */
  path: string;
  /** What was done to it, each once, in the order it first happened. */
  touches: Touch[];
  /** Lines added and removed, over every write, edit and deletion. */
  add: number;
  del: number;
};

type Json = Record<string, unknown>;

const TOUCH: Record<string, Touch> = {
  readFile: "read",
  editFile: "edited",
  writeFile: "written",
  deleteFile: "deleted",
};

/**
 * The files touched by calls that went through, the most recently touched
 * first. A failed call did nothing, so it is not here. A move carries what
 * was done to the old path over to the new one.
 */
export function touchedFiles(blocks: Block[], root: string | null): TouchedFile[] {
  // A Map keeps insertion order: deleting and re-adding moves a file to the end.
  const files = new Map<string, TouchedFile>();
  const touch = (path: string, what: Touch, from?: TouchedFile) => {
    const file = from ?? files.get(path) ?? { path, touches: [], add: 0, del: 0 };
    files.delete(path);
    files.set(path, { ...file, path, touches: file.touches.includes(what) ? file.touches : [...file.touches, what] });
    return files.get(path)!;
  };

  for (const block of blocks) {
    if (block.kind !== "tool" || block.status !== "done") continue;
    const args = parse(block.arguments);
    const result = (block.result ?? {}) as Json;
    if (block.name === "move") {
      // A folder carries a count of its files; this list is of files.
      if (typeof result.files === "number") continue;
      const from = relative(text(result.from) ?? text(args.path), root);
      const to = relative(text(result.to) ?? text(args.newPath), root);
      if (!from || !to) continue;
      const moved = files.get(from);
      files.delete(from);
      touch(to, "moved", moved);
      continue;
    }
    if (block.name === "readFile" && Array.isArray(result.files)) {
      // Several at once: each one that was read, not one that failed.
      for (const file of result.files as Json[]) {
        const path = relative(text(file.path), root);
        if (path && !file.error) touch(path, "read");
      }
      continue;
    }
    const path = relative(text(result.path) ?? text(args.path), root);
    if (!path) continue;
    const what = TOUCH[block.name];
    if (!what) continue;
    const file = touch(path, what);
    const diff = (result.diff ?? {}) as Json;
    file.add += typeof diff.linesAdded === "number" ? diff.linesAdded : 0;
    file.del += typeof diff.linesRemoved === "number" ? diff.linesRemoved : 0;
  }
  return [...files.values()].reverse();
}

function parse(json: string): Json {
  try {
    const value: unknown = JSON.parse(json);
    return value && typeof value === "object" ? (value as Json) : {};
  } catch {
    return {};
  }
}

const text = (value: unknown) => (typeof value === "string" && value.trim() ? value.trim() : undefined);

/** `./src/a.ts` and `/abs/root/src/a.ts` are both `src/a.ts`: one file, one row. */
function relative(path: string | undefined, root: string | null): string | undefined {
  if (!path) return undefined;
  const inside = root && path.startsWith(`${root.replace(/\/+$/, "")}/`) ? path.slice(root.replace(/\/+$/, "").length + 1) : path;
  return inside.replace(/^(\.\/)+/, "") || undefined;
}
