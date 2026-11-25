import { Folder, FolderGit2 } from "lucide-react";
import { KiboBrand } from "./KiboBrand";
import type { RecentFolder } from "../lib/chat";
import "./ChatEmptyState.css";

type Props = {
  /** The open folder's display path, or `null` when nothing is open. */
  workspace: string | null;
  onOpenRepo: () => void;
  /** Folders opened lately, the last one first — offered when nothing is open,
      so the first thing this screen asks for is one press, not a dialog. */
  recent?: RecentFolder[];
  onOpenFolder?: (path: string) => void;
};

/** The last part of a folder's path, as its name is read here. */
const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** How many of the lately opened folders the screen offers: a short list, not
    the whole history — the picker has that. */
const RECENT_SHOWN = 3;

/** Fills the thread while there is nothing to show: the wordmark alone, as a
    start screen's is — a new chat is a place, not a list that is empty. With no
    folder open the screen is about choosing one, and says so. */
export function ChatEmptyState({ workspace, onOpenRepo, recent = [], onOpenFolder }: Props) {
  return (
    <div className="chat-empty">
      <KiboBrand />
      {/* No "New chat" here: this already is one. */}
      {!workspace && (
        <>
          <h2 className="chat-empty-title">Nothing is open</h2>
          {recent.length > 0 && onOpenFolder && (
            <div className="chat-empty-recent">
              {recent.slice(0, RECENT_SHOWN).map((one) => (
                <button
                  key={one.path}
                  className="chat-empty-recent-folder"
                  type="button"
                  title={one.worktreeOf ? `A worktree of ${one.worktreeOf}\n${one.path}` : one.path}
                  onClick={() => onOpenFolder(one.path)}
                >
                  <Folder size={13} />
                  <span>{folderName(one.path)}</span>
                </button>
              ))}
            </div>
          )}
          <div className="chat-empty-actions">
            <button className="btn btn-primary" type="button" onClick={onOpenRepo}>
              <FolderGit2 size={13} />
              Open folder…
            </button>
          </div>
        </>
      )}
    </div>
  );
}
