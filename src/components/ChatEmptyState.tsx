import { FolderGit2 } from "lucide-react";
import logo from "../assets/kibo-logo.png";
import { UsageStrip } from "./UsageHeatmap";
import "./ChatEmptyState.css";

type Props = {
  /** The open folder's display path, or `null` when nothing is open. */
  workspace: string | null;
  onOpenRepo: () => void;
};

/** Fills the thread while there is nothing to show: no workspace, or no messages. */
export function ChatEmptyState({ workspace, onOpenRepo }: Props) {
  const name = workspace?.split("/").filter(Boolean).pop() ?? "";

  return (
    <div className="chat-empty">
      <img className="chat-empty-logo" src={logo} alt="" />
      <h2 className="chat-empty-title">
        {workspace ? "Start the conversation" : "Nothing is open"}
      </h2>
      <p className="chat-empty-text">
        {workspace
          ? `Describe a task and the agent works through ${name}, showing every tool call as it goes.`
          : "Open a folder to give the agent a workspace. It reads and writes inside that folder; a command it runs is not confined to it, which is what the approval prompts are for."}
      </p>
      {/* No "New chat" here: this already is one. */}
      {!workspace && (
        <div className="chat-empty-actions">
          <button className="btn btn-primary" type="button" onClick={onOpenRepo}>
            <FolderGit2 size={13} />
            Open folder…
          </button>
        </div>
      )}
      {workspace && (
        <div className="chat-empty-usage">
          <UsageStrip />
        </div>
      )}
      <p className="chat-empty-hint">
        <kbd>Enter</kbd> to send
        <span className="sep">·</span>
        <kbd>Shift</kbd>
        <span className="plus">+</span>
        <kbd>Enter</kbd> for a new line
      </p>
    </div>
  );
}
