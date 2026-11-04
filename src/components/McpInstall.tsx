import { useState } from "react";
import { Dropdown } from "./Dropdown";
import { Field } from "./EntryField";
import {
  EMPTY_SERVER,
  pastedServers,
  readMcpServer,
  writeMcpServer,
  type McpServerFields,
  type PastedServer,
} from "../lib/configEntries";
import { isObject, parseFile } from "../lib/configSnippets";
import "./McpInstall.css";

// Installing a server, or editing one: a snippet pasted from a README fills the
// fields, the fields sit in two columns, and the last step shows the entry that
// goes into mcp.json. What is written is the file the JSON editor edits — the
// screen is a way in, not a second copy.

const TRANSPORTS: Record<McpServerFields["transport"], string> = { command: "A command", url: "At a URL" };
const RESULTS = { cleared: "Cleared when the context fills", kept: "Kept" };
const EXPOSURES: Record<McpServerFields["exposure"], string> = {
  direct: "Declared to the model",
  deferred: "Found with toolSearch",
  hidden: "Hidden",
};

type Step = "paste" | "connection" | "review";

// In the order the screen walks them.
const STEPS: { id: Step; label: string }[] = [
  { id: "paste", label: "Paste" },
  { id: "connection", label: "Connection" },
  { id: "review", label: "Review" },
];

type Props = {
  /** The server being edited, `null` for a new one. */
  name: string | null;
  /** The file as it stands on disk. */
  text: string | undefined;
  /** Where the file lives, for the review. */
  path: string;
  /** What the backend said about the last save. */
  error: string | null;
  /** Resolves to whether it was stored. */
  onSave: (text: string) => Promise<boolean>;
  onClose: () => void;
  /** Leaves the screen for the whole file. */
  onEditJson: () => void;
};

/** How a server runs, on one line, for the list a paste of several makes. */
const commandLine = (fields: McpServerFields) =>
  fields.transport === "url" ? fields.url : [fields.command, ...fields.args.split("\n")].filter(Boolean).join(" ");

export function McpInstall({ name, text, path, error, onSave, onClose, onEditJson }: Props) {
  const [step, setStep] = useState<Step>(name === null ? "paste" : "connection");
  const [fields, setFields] = useState<McpServerFields>(
    () => (name !== null && text ? readMcpServer(text, name) : null) ?? EMPTY_SERVER,
  );
  const [draft, setDraft] = useState("");
  // What the last paste held, or why it held nothing.
  const [found, setFound] = useState<{ servers: PastedServer[] } | { error: string } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const file = parseFile(text ?? "");
  const inFile = file && isObject(file.mcpServers) ? file.mcpServers : {};
  const typed = fields.name.trim();
  // A pasted name that is already in the file replaces that entry where it
  // stands, as the file editor's own paste does.
  const previous = name ?? (typed && typed in inFile ? typed : null);
  const written = writeMcpServer(text ?? "", previous, fields);
  const writtenFile = "text" in written ? parseFile(written.text) : null;
  const entry = writtenFile && isObject(writtenFile.mcpServers) ? writtenFile.mcpServers[typed] : undefined;

  const steps = name === null ? STEPS : STEPS.filter((s) => s.id !== "paste");
  const at = steps.findIndex((s) => s.id === step);
  const go = (next: Step) => {
    setProblem(null);
    setStep(next);
  };

  const pick = (server: PastedServer) => {
    setFields(server.fields);
    go("connection");
  };

  // A paste that holds one server goes on by itself; several are listed to
  // choose from, since the fields hold one.
  const read = () => {
    const result = pastedServers(draft);
    setFound(result);
    if ("servers" in result && result.servers.length === 1) pick(result.servers[0]);
  };

  const next = () => {
    if ("error" in written) return setProblem(written.error);
    go("review");
  };

  const install = async () => {
    if ("error" in written) return setProblem(written.error);
    setSaving(true);
    const stored = await onSave(written.text);
    setSaving(false);
    if (stored) onClose();
  };

  const set = (key: keyof McpServerFields) => (e: { target: { value: string } }) => {
    setProblem(null);
    setFields((f) => ({ ...f, [key]: e.target.value }));
  };

  return (
    <div className="mcp-install">
      <nav className="mcp-install-steps" aria-label="Steps">
        {steps.map(({ id, label }, i) => (
          <button
            key={id}
            type="button"
            className="mcp-install-step"
            aria-current={step === id ? "step" : undefined}
            disabled={i > at}
            onClick={() => go(id)}
          >
            <span className="mcp-install-step-num">{i + 1}</span>
            {label}
          </button>
        ))}
      </nav>

      {step === "paste" && (
        <div className="mcp-install-paste">
          <p className="modal-note">
            Paste a server as its README prints it: a whole <code>{'{"mcpServers": {…}}'}</code>, a bare{" "}
            <code>{'{"name": {…}}'}</code>, or the entry alone. What is read fills the fields next step.
          </p>
          <textarea
            className="mcp-install-text"
            aria-label="MCP server JSON"
            spellCheck={false}
            rows={12}
            placeholder={'{\n  "mcpServers": {\n    "github": { "command": "npx", "args": ["-y", "server-github"] }\n  }\n}'}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          {found && "error" in found && <p className="entry-error">{found.error}</p>}
          {found && "servers" in found && found.servers.length > 1 && (
            <div className="mcp-install-found">
              <p className="modal-note">
                The paste holds {found.servers.length} servers. Set one up here; to add them all as they are, paste
                them into the file's JSON editor.
              </p>
              {found.servers.map((server, i) => (
                <button
                  key={`${server.name}/${i}`}
                  type="button"
                  className="mcp-install-found-row"
                  aria-label={`Set up ${server.name || "the unnamed server"}`}
                  onClick={() => pick(server)}
                >
                  <b>{server.name || "No name"}</b>
                  <span className="mono">{commandLine(server.fields)}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {step === "connection" && (
        <>
          <div className="mcp-install-columns">
            <section className="mcp-install-col" aria-label="How it runs">
              <h4 className="mcp-install-head">How it runs</h4>
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
            </section>

            <section className="mcp-install-col" aria-label="How the agent uses it">
              <h4 className="mcp-install-head">How the agent uses it</h4>
              {fields.transport === "command" && (
                <Field label="Environment" hint="KEY=VALUE, one per line. Stored in the file as it is — it may hold tokens.">
                  <textarea className="entry-input mono" rows={2} value={fields.env} onChange={set("env")} spellCheck={false} />
                </Field>
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
            </section>
          </div>
          {problem && <p className="entry-error">{problem}</p>}
        </>
      )}

      {step === "review" && (
        <>
          <p className="mcp-install-note">
            {previous !== null ? (
              <>
                Replaces the server <b>{typed || previous}</b> in <span className="mono">{path || "the MCP file"}</span>,
                where it stands.
              </>
            ) : (
              <>
                Adds the server <b>{typed}</b> to <span className="mono">{path || "the MCP file"}</span>.
              </>
            )}
          </p>
          {error && <p className="entry-error">{error}</p>}
          {entry !== undefined && <pre className="mcp-install-entry">{JSON.stringify(entry, null, 2)}</pre>}
          <p className="modal-note">
            The app writes {path || "the file"} in its own layout: spacing and key order typed there by hand are not
            kept. It starts with the next Agent turn.
          </p>
        </>
      )}

      <div className="mcp-install-actions">
        {step === "paste" && (
          <>
            <button className="btn btn-primary" type="button" disabled={!draft.trim()} onClick={read}>
              Continue
            </button>
            <button
              className="btn btn-ghost"
              type="button"
              onClick={() => {
                setFields(EMPTY_SERVER);
                go("connection");
              }}
            >
              Start from scratch
            </button>
          </>
        )}
        {step === "connection" && (
          <>
            <button className="btn btn-primary" type="button" onClick={next}>
              Next
            </button>
            {at > 0 && (
              <button className="btn btn-ghost" type="button" onClick={() => go(steps[at - 1].id)}>
                Back
              </button>
            )}
          </>
        )}
        {step === "review" && (
          <>
            <button className="btn btn-primary" type="button" disabled={saving} onClick={() => void install()}>
              {name !== null ? "Save" : previous !== null ? "Replace" : "Install"}
            </button>
            <button className="btn btn-ghost" type="button" onClick={() => go("connection")}>
              Back
            </button>
          </>
        )}
        <button className="link-btn mcp-install-json" type="button" onClick={onEditJson}>
          Edit as JSON
        </button>
      </div>
    </div>
  );
}
