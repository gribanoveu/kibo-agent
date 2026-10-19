import { Modal } from "./Modal";
import type { WorktreeCheck } from "../lib/git";
import "./WorktreeRemoveDialog.css";

type Props = {
  /** The worktree to remove and what was found in it; `null` when closed. */
  asked: { path: string; check: WorktreeCheck } | null;
  onConfirm: () => void;
  onClose: () => void;
};

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** What removing the open worktree deletes and keeps — or why it will not. */
export function WorktreeRemoveDialog({ asked, onConfirm, onClose }: Props) {
  const check = asked?.check;
  // Refused before anyone clicks: the backend would refuse too.
  const dirty = (check?.dirty.length ?? 0) > 0;
  const loose = check !== undefined && check.branch === null && check.ownCommits > 0;
  const refused = dirty || loose;

  return (
    <Modal
      title={refused ? "This worktree has work in it" : "Remove this worktree?"}
      open={asked !== null}
      onClose={onClose}
      footer={
        refused ? (
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Close
          </button>
        ) : (
          <>
            <button type="button" className="btn btn-ghost" onClick={onClose}>
              Cancel
            </button>
            <button type="button" className="btn btn-primary" onClick={onConfirm}>
              Remove
            </button>
          </>
        )
      }
    >
      {asked && check && dirty && (
        <>
          <p className="worktree-remove-text">Uncommitted changes would be lost with the folder:</p>
          <ul className="worktree-remove-paths">
            {check.dirty.map((path) => (
              <li key={path}>{path}</li>
            ))}
          </ul>
          <p className="worktree-remove-text">
            Nothing was removed. Commit or discard them first — or ask the agent to.
          </p>
        </>
      )}
      {asked && check && !dirty && loose && (
        <p className="worktree-remove-text">
          {check.ownCommits} {check.ownCommits === 1 ? "commit is" : "commits are"} on no branch and would be lost.
          Nothing was removed. Make a branch for them first — or ask the agent to.
        </p>
      )}
      {asked && check && !refused && (
        <ul className="worktree-remove-facts">
          <li>
            The folder <b>{folderName(asked.path)}</b>, a worktree of <b>{folderName(check.main)}</b>, is deleted.
          </li>
          <li>
            {check.chats === 0
              ? "It has no chats."
              : `${check.chats === 1 ? "Its chat is" : `Its ${check.chats} chats are`} deleted with it.`}
          </li>
          {check.branch && (
            <li>
              {check.keepsBranch ? (
                <>
                  Branch <b>{check.branch}</b> is kept
                  {check.ownCommits > 0
                    ? ` — ${check.ownCommits} ${check.ownCommits === 1 ? "commit is" : "commits are"} only on it.`
                    : " — it is not one Kibo made."}
                </>
              ) : (
                <>
                  Branch <b>{check.branch}</b> is deleted — everything on it is on other branches too.
                </>
              )}
            </li>
          )}
        </ul>
      )}
    </Modal>
  );
}
