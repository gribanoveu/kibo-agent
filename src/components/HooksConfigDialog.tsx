import { Modal } from "./Modal";
import { HookForm } from "./ConfigEntryForm";
import { ConfigFileEditor } from "./ConfigFileEditor";
import { mergeHooks } from "../lib/configSnippets";
import type { HooksView } from "../lib/agentHooks";
import type { EntryDialog } from "../hooks/useConfigDialogs";

/** The hooks file's dialog: one hook's form, or the whole file as JSON. */
export function HooksConfigDialog({
  dialog,
  view,
  error,
  onSave,
  onClose,
  onEditJson,
}: {
  dialog: EntryDialog<number>;
  view: HooksView | null;
  error: string | null;
  onSave: (text: string) => Promise<boolean>;
  onClose: () => void;
  onEditJson: () => void;
}) {
  return (
    <Modal
      title={dialog === "json" || dialog === null ? "Hooks" : dialog.entry === null ? "Add a hook" : "Edit hook"}
      wide={dialog === "json"}
      open={dialog !== null}
      onClose={onClose}
    >
      {dialog !== null && dialog !== "json" ? (
        <HookForm
          key={dialog.entry ?? -1}
          index={dialog.entry}
          text={view?.text}
          error={error}
          onSave={onSave}
          onClose={onClose}
          onEditJson={onEditJson}
        />
      ) : (
      <ConfigFileEditor
        label="Hooks configuration"
        merge={mergeHooks}
        text={view?.text}
        error={error}
        onSave={onSave}
        onClose={onClose}
        note={
          <>
            Claude Code's <code>hooks</code> format: <code>PreToolUse</code>, <code>PostToolUse</code> and{" "}
            <code>Stop</code> run here. The command gets the event as JSON on stdin; exit code 2 refuses the call,
            or sends the agent back from Stop, with stderr as the reason. A <code>matcher</code> names this app's
            tools — <code>runCommand</code>, <code>editFile</code> — not Claude Code's. Kept in{" "}
            {view?.path || "the app directory"}.
          </>
        }
      />
      )}
    </Modal>
  );
}