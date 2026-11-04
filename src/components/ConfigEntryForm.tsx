import { useState } from "react";
import { Dropdown } from "./Dropdown";
import { Field } from "./EntryField";
import { EMPTY_HOOK, readHook, writeHook, type HookFields, type Written } from "../lib/configEntries";
import "./ConfigEntryForm.css";

// One hook as a form, for the times a snippet is not at hand. What it writes is
// the same file the JSON editor edits: the form is a way in, not a second copy.
// A server is set up by `McpInstall`, which reads a pasted entry and has steps.

type Common = {
  /** The file as it stands on disk. */
  text: string | undefined;
  /** What the backend said about the last save. */
  error: string | null;
  /** Resolves to whether it was stored. */
  onSave: (text: string) => Promise<boolean>;
  onClose: () => void;
  /** Leaves the form for the whole file. */
  onEditJson: () => void;
};

function Actions({
  problem,
  error,
  onClose,
  onEditJson,
}: {
  problem: string | null;
  error: string | null;
  onClose: () => void;
  onEditJson: () => void;
}) {
  return (
    <>
      {(problem || error) && <p className="entry-error">{problem ?? error}</p>}
      <div className="entry-actions">
        <button className="btn btn-primary" type="submit">
          Save
        </button>
        <button className="btn btn-ghost" type="button" onClick={onClose}>
          Cancel
        </button>
        <button className="link-btn entry-json" type="button" onClick={onEditJson}>
          Edit as JSON
        </button>
      </div>
    </>
  );
}

/** Writes the entry into the file and saves it; a problem the form can see stops it before the backend is asked. */
function useSubmit(text: string | undefined, write: (text: string) => Written, save: Common["onSave"], close: () => void) {
  const [problem, setProblem] = useState<string | null>(null);
  const submit = async () => {
    const written = write(text ?? "");
    if ("error" in written) return setProblem(written.error);
    setProblem(null);
    if (await save(written.text)) close();
  };
  return { problem, submit };
}

/** What a matcher can name here — this app's tools, not Claude Code's. */
const TOOL_NAMES = "runCommand, editFile, writeFile, delete, move, readFile, grep, listFiles";

/** `index` is the hook's row in the Hooks tab, `null` for a new one. */
export function HookForm({ index, text, error, onSave, onClose, onEditJson }: Common & { index: number | null }) {
  const [fields, setFields] = useState<HookFields>(
    () => (index !== null && text ? readHook(text, index) : null) ?? EMPTY_HOOK,
  );
  const set = (key: keyof HookFields) => (e: { target: { value: string } }) =>
    setFields((f) => ({ ...f, [key]: e.target.value }));
  const { problem, submit } = useSubmit(text, (t) => writeHook(t, index, fields), onSave, onClose);

  return (
    <form
      className="entry-form"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <Field label="When" menu>
        <Dropdown
          below
          title="Event"
          label={fields.event}
          value={fields.event}
          options={[
            { value: "PreToolUse", hint: "Before a tool call; exit code 2 refuses it" },
            { value: "PostToolUse", hint: "After a tool call that succeeded" },
            { value: "Stop", hint: "When the agent finishes; exit code 2 sends it back" },
          ]}
          onPick={(event) => setFields((f) => ({ ...f, event }))}
        />
      </Field>
      {fields.event !== "Stop" && (
        <Field label="Tools" hint={<>A regular expression over the tool name; empty for every tool. Here: {TOOL_NAMES}…</>}>
          <input className="entry-input mono" value={fields.matcher} onChange={set("matcher")} placeholder="runCommand|editFile" />
        </Field>
      )}
      <Field label="Command" hint="Gets the event as JSON on stdin. Exit code 2 blocks, with stderr as the reason.">
        <input className="entry-input mono" value={fields.command} onChange={set("command")} autoFocus={index === null} />
      </Field>
      <div className="entry-row">
        <Field label="Timeout, s" hint="60 when empty.">
          <input className="entry-input" inputMode="numeric" value={fields.timeout} onChange={set("timeout")} />
        </Field>
      </div>
      <Actions problem={problem} error={error} onClose={onClose} onEditJson={onEditJson} />
    </form>
  );
}
