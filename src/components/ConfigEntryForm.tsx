import { useState, type ReactNode } from "react";
import { Dropdown } from "./Dropdown";
import {
  EMPTY_HOOK,
  EMPTY_SERVER,
  readHook,
  readMcpServer,
  writeHook,
  writeMcpServer,
  type HookFields,
  type McpServerFields,
  type Written,
} from "../lib/configEntries";
import "./ConfigEntryForm.css";

// One server or one hook as a form, for the times a snippet is not at hand.
// What it writes is the same file the JSON editor edits: the form is a way
// in, not a second copy.

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

/**
 * A labelled input. `menu` for a Dropdown: a `<label>` around one would pass
 * its own click on to the trigger and shut the menu it just opened.
 */
function Field({ label, hint, menu, children }: { label: string; hint?: ReactNode; menu?: boolean; children: ReactNode }) {
  const Wrap = menu ? "div" : "label";
  return (
    <Wrap className="entry-field">
      <span className="entry-label">{label}</span>
      {children}
      {hint && <span className="entry-hint">{hint}</span>}
    </Wrap>
  );
}

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

const TRANSPORTS: Record<McpServerFields["transport"], string> = { command: "A command", url: "At a URL" };
const RESULTS = { cleared: "Cleared when the context fills", kept: "Kept" };
const EXPOSURES: Record<McpServerFields["exposure"], string> = {
  direct: "Declared to the model",
  deferred: "Found with toolSearch",
  hidden: "Hidden",
};

/** `name` is the server being edited, `null` for a new one. */
export function McpServerForm({ name, text, error, onSave, onClose, onEditJson }: Common & { name: string | null }) {
  const [fields, setFields] = useState<McpServerFields>(
    () => (name !== null && text ? readMcpServer(text, name) : null) ?? EMPTY_SERVER,
  );
  const set = (key: keyof McpServerFields) => (e: { target: { value: string } }) =>
    setFields((f) => ({ ...f, [key]: e.target.value }));
  const { problem, submit } = useSubmit(text, (t) => writeMcpServer(t, name, fields), onSave, onClose);

  return (
    <form
      className="entry-form"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <Field label="Name" hint="How the agent's tools are named: mcp__name__tool.">
        <input className="entry-input" value={fields.name} onChange={set("name")} autoFocus={name === null} />
      </Field>
      <Field label="Runs" menu>
        <Dropdown
          below
          title="How it runs"
          label={TRANSPORTS[fields.transport]}
          value={fields.transport}
          options={[
            { value: "command", label: TRANSPORTS.command, hint: "A program on this machine, spoken to over stdio" },
            { value: "url", label: TRANSPORTS.url, hint: "A server somewhere else, over Streamable HTTP" },
          ]}
          onPick={(transport) => setFields((f) => ({ ...f, transport: transport as McpServerFields["transport"] }))}
        />
      </Field>
      {fields.transport === "command" ? (
        <>
          <Field label="Command" hint="A whole command line works too — npx -y server-github is split into command and args.">
            <input className="entry-input mono" value={fields.command} onChange={set("command")} placeholder="npx" />
          </Field>
          <Field label="Arguments" hint="One per line.">
            <textarea className="entry-input mono" rows={3} value={fields.args} onChange={set("args")} />
          </Field>
          <Field label="Environment" hint="KEY=VALUE, one per line. Stored in the file as it is — it may hold tokens.">
            <textarea className="entry-input mono" rows={2} value={fields.env} onChange={set("env")} spellCheck={false} />
          </Field>
        </>
      ) : (
        <>
          <Field label="URL" hint="Plain http:// is safe only to this machine: on the way anywhere else, the headers can be read.">
            <input className="entry-input mono" value={fields.url} onChange={set("url")} placeholder="https://example.com/mcp" />
          </Field>
          <Field label="Headers" hint="Name: value, one per line — Authorization: Bearer … for a token. Stored in the file as it is.">
            <textarea className="entry-input mono" rows={2} value={fields.headers} onChange={set("headers")} spellCheck={false} />
          </Field>
        </>
      )}
      <div className="entry-row">
        <Field label="Weight" hint="Cost of a call in the turn's budget. 3 when empty.">
          <input className="entry-input" inputMode="numeric" value={fields.weight} onChange={set("weight")} />
        </Field>
        <Field label="Timeout, s" hint="120 when empty.">
          <input className="entry-input" inputMode="numeric" value={fields.timeoutSecs} onChange={set("timeoutSecs")} />
        </Field>
      </div>
      <Field label="Its tools" menu hint="Single tools are switched in the server's row in the MCP tab.">
        <Dropdown
          below
          title="How the model reaches its tools"
          label={EXPOSURES[fields.exposure]}
          value={fields.exposure}
          options={[
            { value: "direct", label: EXPOSURES.direct, hint: "In every request, like a built-in tool" },
            {
              value: "deferred",
              label: EXPOSURES.deferred,
              hint: "Only the server's name in every request; the model searches for the tools it needs",
            },
            { value: "hidden", label: EXPOSURES.hidden, hint: "None, but those switched on in the tab" },
          ]}
          onPick={(exposure) => setFields((f) => ({ ...f, exposure: exposure as McpServerFields["exposure"] }))}
        />
      </Field>
      <Field label="Its results" menu hint="Keep them for a server that answers with templates or rules the model follows.">
        <Dropdown
          below
          title="What happens to its results as the context fills"
          label={fields.keepResults ? RESULTS.kept : RESULTS.cleared}
          value={fields.keepResults ? "kept" : "cleared"}
          options={[
            { value: "cleared", label: RESULTS.cleared, hint: "Old ones are cleared like any tool's, and called again when needed" },
            { value: "kept", label: RESULTS.kept, hint: "Never cleared — for instructions rather than data" },
          ]}
          onPick={(value) => setFields((f) => ({ ...f, keepResults: value === "kept" }))}
        />
      </Field>
      <Actions problem={problem} error={error} onClose={onClose} onEditJson={onEditJson} />
    </form>
  );
}

/** What a matcher can name here — this app's tools, not Claude Code's. */
const TOOL_NAMES = "runCommand, editFile, writeFile, deleteFile, move, readFile, grep, listFiles";

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
