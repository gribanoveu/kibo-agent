import { Modal } from "./Modal";
import type { SwitchBlock } from "../hooks/useFolderSwitch";
import "./FolderSwitchDialog.css";

type Props = {
  blocked: SwitchBlock | null;
  /** The active folder — where the turn that holds the window works. */
  folder: string | null;
  onStopAgent: () => void;
  onClose: () => void;
};

const nameOf = (path: string | null | undefined) => path?.split(/[\\/]/).filter(Boolean).pop();

/** Says why the active folder cannot change yet, or what closing a folder would end, before it does. */
export function FolderSwitchDialog({ blocked, folder, onStopAgent, onClose }: Props) {
  const name = nameOf(folder) ?? "this folder";
  const closing = blocked?.kind === "processes" ? blocked.folders.map((path) => nameOf(path) ?? path).join(", ") : "";
  const agent = blocked?.kind === "agent";
  // A paused turn is ended by answering its card, not by Stop.
  const waiting = blocked?.kind === "agent" && blocked.waiting;

  return (
    <Modal
      title={waiting ? "The agent is waiting for you" : agent ? "The agent is still working" : "Stop what runs here?"}
      open={blocked !== null}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          {!waiting && (
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (blocked?.kind === "agent") onStopAgent();
                else blocked?.go();
                onClose();
              }}
            >
              {agent ? "Stop the agent" : "Stop them and close"}
            </button>
          )}
        </>
      }
    >
      {waiting ? (
        <p className="switch-text">
          It is waiting for your answer on a card in <b>{name}</b>, and its chat is saved there when the turn ends.
          Answer the card, then change folders.
        </p>
      ) : blocked?.kind === "agent" ? (
        <p className="switch-text">
          It is working in <b>{name}</b>, and its chat is saved there when it finishes. Stop it first, then change
          folders.
        </p>
      ) : (
        blocked && (
          <>
            <p className="switch-text">
              Closing <b>{closing}</b> stops what runs in it:
            </p>
            <ul className="switch-processes">
              {blocked.processes.map((p) => (
                <li key={`p${p.id}`}>{p.command}</li>
              ))}
              {blocked.terminals.map((t) => (
                <li key={`t${t.id}`}>terminal: {t.shell}</li>
              ))}
            </ul>
          </>
        )
      )}
    </Modal>
  );
}
