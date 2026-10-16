import { invoke } from "@tauri-apps/api/core";
import { requireBackend } from "./backend";

// The open folder's tree, for the Files tab.

/** Mirrors `domain::file_tree`. A deleted file is not listed: it is not on disk. */
export type FileStatus = "modified" | "added" | "untracked" | "conflicted";
export type TreeEntry = { name: string; path: string; isDir: boolean; status: FileStatus | null; changed: boolean };
export type FolderListing = { entries: TreeEntry[]; more: number };

/** One folder of the open one; `""` is the open folder itself. */
export async function workspaceList(dir: string): Promise<FolderListing> {
  requireBackend();
  return invoke<FolderListing>("workspace_list", { dir });
}
