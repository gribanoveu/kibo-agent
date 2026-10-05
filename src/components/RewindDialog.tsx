import { Modal } from "./Modal";
import type { RewindSkip } from "../lib/chat";
import type { RewindAsk } from "../hooks/useRewind";
import "./RewindDialog.css";

type Props = {
  /** What the rewind would do; `null` when closed. */
  asked: RewindAsk | null;
  /** A rewind with a summary is waiting for the model. */
  summarizing?: boolean;
  /** `summarize`: tell the agent what was tried in the part cut off. */
  onConfirm: (summarize: boolean) => void;
  onClose: () => void;
};

/** Why a file stays as it is, in the user's terms. */
export const SKIP_TEXT: Record<RewindSkip, string> = {
  notKept: "too large to have been kept",
  expired: "its copy was cleared after 30 days",
  changedBetween: "changed by something else between the agent's edits",
  changedSince: "changed since — left as it is",
  unsafePath: "outside the folder",
  writeFailed: "could not be written",
};

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What rewinding the chat to before a message does to it and to the files. */
export function RewindDialog({ asked, summarizing = false, onConfirm, onClose }: Props) {
  const files = asked?.files ?? [];
  const ready = files.filter((file) => file.skip === null).length;
  return (
    <Modal
      title="Rewind to before this message?"
      open={asked !== null}
      onClose={onClose}
      wide={files.length > 0}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={summarizing}>
            Cancel
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => onConfirm(true)} disabled={summarizing}>
            {summarizing ? "Summarizing…" : "Rewind with a summary"}
          </button>
          <button type="button" className="btn btn-primary" onClick={() => onConfirm(false)} disabled={summarizing}>
            Rewind
          </button>
        </>
      }
    >
      <p className="rewind-text">
        The messages from this one on are removed from the chat, and this one goes back to the message box to change and
        send again. With a summary, the agent keeps a short account of what was tried from here on and why it did not
        work — one request to the model.
      </p>
      {files.length === 0 ? (
        <p className="rewind-text">The agent changed no files after it.</p>
      ) : (
        <>
          <p className="rewind-text">
            Files the agent changed after it go back to how they were:{" "}
            <b>
              {ready} ready · {files.length - ready} left as they are
            </b>
          </p>
          <ul className="rewind-files">
            {files.map((file) => (
              <li key={file.path} className={file.skip ? "skipped" : undefined}>
                <span className="rewind-path">{file.path}</span>
                <span className="rewind-action">{file.action}</span>
                <span className="rewind-status">{file.skip ? SKIP_TEXT[file.skip] : "ready"}</span>
              </li>
            ))}
          </ul>
        </>
      )}
      {asked && asked.unrecorded > 0 && (
        <p className="rewind-text">
          {plural(asked.unrecorded, "command or external tool", "commands or external tools")} ran after it — whatever{" "}
          {asked.unrecorded === 1 ? "it" : "they"} changed stays as it is.
        </p>
      )}
    </Modal>
  );
}
