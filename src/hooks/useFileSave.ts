import { useRef, useState } from "react";
import { fileWrite, type FileTarget, type FileView } from "../lib/chat";
import { sameText, type Edits } from "./useOpenFiles";

/**
 * Writing the file viewer's edits: one save at a time, the files whose last
 * save did not land, and why. The viewer decides when to save — a pause, a
 * blur, a tab or the window closing — and this does the writing.
 */
export function useFileSave(
  workspace: string | null | undefined,
  edits: Edits | undefined,
  reload: (file: FileTarget) => Promise<FileView | null>,
) {
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
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
    if (sameText(from, text)) {
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
        const saved = (await fileWrite(workspace ?? "", path, from, text)).kind === "saved";
        const now = await reload(file);
        if (saved) {
          unsaved.current.delete(path);
          // Typed on while it was written: what came after stays a draft, now
          // over the text just saved. A tab not on screen had no one typing.
          const disk = now && !now.unviewable ? now.new : null;
          if (disk !== null) edits?.rebase(path, disk);
          else edits?.drop(path);
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
  return { saving, saveError, unsaved, inflight, persist };
}
