import { Modal } from "./Modal";
import type { FolderTrust } from "../lib/chat";
import "./FolderTrustDialog.css";

type Props = {
  trust: FolderTrust | null;
  open: boolean;
  error: string | null;
  onDecide: (trusted: boolean) => void;
  /** Leaves it unanswered: asked again next time the folder opens. */
  onClose: () => void;
};

/** Asks whether the open folder's own skills and `/` commands may be loaded. */
export function FolderTrustDialog({ trust, open, error, onDecide, onClose }: Props) {
  const name = trust?.folder.split(/[\\/]/).filter(Boolean).pop() ?? "this folder";
  return (
    <Modal
      title="Trust this folder?"
      open={open && trust !== null}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={() => onDecide(false)}>
            Don't trust
          </button>
          <button type="button" className="btn btn-primary" onClick={() => onDecide(true)}>
            Trust
          </button>
        </>
      }
    >
      <p className="trust-text">
        {trust?.needed ? (
          <>
            <b>{name}</b> has skills or <code>/</code> commands of its own, in <code>.claude/skills</code>,{" "}
            <code>.agents/skills</code> or <code>.kibo/commands</code>.
          </>
        ) : (
          <>
            <b>{name}</b> has no skills or <code>/</code> commands of its own yet.
          </>
        )}{" "}
        Whoever wrote the repository wrote them for the agent, and a skill can tell it to run a script from the
        repository. They are loaded only in a folder you trust; the answer covers the folders inside it.
      </p>
      <p className="trust-text">
        Trusting it does not limit what the agent's tools can do, and does not protect against instructions hidden in
        the folder's files. <code>AGENTS.md</code> and <code>CLAUDE.md</code> are read either way.
      </p>
      {trust?.trusted != null && <p className="trust-text">Now: {trust.trusted ? "trusted" : "not trusted"}.</p>}
      {error && <p className="trust-error">{error}</p>}
    </Modal>
  );
}
