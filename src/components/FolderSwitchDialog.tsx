import { Modal } from "./Modal";
import type { SwitchBlock } from "../hooks/useFolderSwitch";
import "./FolderSwitchDialog.css";

type Props = {
  blocked: SwitchBlock | null;
  /** Stops the turns of these chats — the ones working in the folder that closes. */
  onStopAgent: (chats: string[]) => void;
  onClose: () => void;
};

const nameOf = (path: string | null | undefined) => path?.split(/[\\/]/).filter(Boolean).pop();

/** Says why a folder cannot close yet, or what closing it would end, before it does. */
export function FolderSwitchDialog({ blocked, onStopAgent, onClose }: Props) {
  const name = blocked?.kind === "agent" ? (nameOf(blocked.folder) ?? blocked.folder) : "";
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
                if (blocked?.kind === "agent") onStopAgent(blocked.chats);
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
          A turn in <b>{name}</b> is waiting for your answer on a card. Answer the card, then close the folder.
        </p>
      ) : blocked?.kind === "agent" ? (
        <p className="switch-text">
          It is working in <b>{name}</b>. Stop it first, then close the folder.
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
