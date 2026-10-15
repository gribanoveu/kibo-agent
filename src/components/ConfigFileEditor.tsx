import { useEffect, useState, type ClipboardEvent, type KeyboardEvent, type ReactNode } from "react";
import { jsonError, type Merged } from "../lib/configSnippets";
import { matches } from "../lib/shortcuts";
import "./ConfigFileEditor.css";

// A settings file edited whole, in one box, because its format is one that
// others already write — an MCP server's README, a hook shared for Claude
// Code: pasting that snippet is the fastest way in, and a field-by-field form
// would have to be translated back into it.

type Props = {
  /** Names the box for assistive technology. */
  label: string;
  /** The file as it stands on disk. */
  text: string | undefined;
  error: string | null;
  /** What the format is and where the file lives. */
  note: ReactNode;
  /**
   * Joins a pasted snippet to the file, or `null` for text that is not one —
   * which is then pasted as it is.
   */
  merge?: (current: string, pasted: string) => Merged | null;
  /** Resolves to whether it was stored. */
  onSave: (text: string) => Promise<boolean>;
  onClose: () => void;
};

const INDENT = "  ";

export function ConfigFileEditor({ label, text, error, note, merge, onSave, onClose }: Props) {
  const [draft, setDraft] = useState(text ?? "");
  // Follows the file until the user starts typing, so the box is never a
  // stale copy of what is on disk.
  const [touched, setTouched] = useState(false);
  // What the last paste did to the file, until the next keystroke.
  const [merged, setMerged] = useState<string | null>(null);
  useEffect(() => {
    if (!touched) setDraft(text ?? "");
  }, [text, touched]);

  const problem = draft.trim() ? jsonError(draft) : null;

  const edit = (next: string, message: string | null = null) => {
    setDraft(next);
    setTouched(true);
    setMerged(message);
  };

  const save = async () => {
    if (problem) return;
    if (await onSave(draft)) {
      setTouched(false);
      onClose();
    }
  };

  // A snippet joins the file rather than landing wherever the caret was. Not
  // when everything is selected: pasting over all of it means "this instead".
  const paste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const box = e.currentTarget;
    if (!merge || (box.selectionStart === 0 && box.selectionEnd === box.value.length && box.value)) return;
    const result = merge(draft, e.clipboardData.getData("text"));
    if (!result) return;
    e.preventDefault();
    edit(result.text, result.message);
  };

  const keys = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (matches(e, "save")) {
      e.preventDefault();
      void save();
    } else if (matches(e, "indent")) {
      // An indent, as in any editor. Shift+Tab still leaves the box.
      // `insertText` keeps the edit on the undo stack; a plain value change would not.
      e.preventDefault();
      if (!document.execCommand?.("insertText", false, INDENT)) {
        const { selectionStart: from, selectionEnd: to, value } = e.currentTarget;
        edit(value.slice(0, from) + INDENT + value.slice(to));
      }
    }
  };

  return (
    <div className="config-file">
      <div className="config-file-main">
        <textarea
          className="config-file-text"
          aria-label={label}
          aria-invalid={problem ? true : undefined}
          spellCheck={false}
          value={draft}
          onChange={(e) => edit(e.target.value)}
          onPaste={paste}
          onKeyDown={keys}
        />
        {problem ? (
          <p className="config-file-status bad" role="status">
            {problem.line ? `Line ${problem.line}, column ${problem.column}: ` : "Not valid JSON: "}
            {problem.message}
          </p>
        ) : merged ? (
          <p className="config-file-status" role="status">
            {merged}
          </p>
        ) : (
          merge && <p className="config-file-status hint">Paste a snippet anywhere — it is added to the file.</p>
        )}
        {error && <p className="modal-note config-file-error">{error}</p>}
      </div>

      <aside className="config-file-side">
        <p className="modal-note">{note}</p>
      </aside>

      <div className="config-file-actions">
        <button
          className="btn btn-primary"
          type="button"
          disabled={!!problem}
          title={problem ? "Fix the JSON first" : "Save (⌘S)"}
          onClick={save}
        >
          Save
        </button>
        <button className="btn btn-ghost" type="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    </div>
  );
}
