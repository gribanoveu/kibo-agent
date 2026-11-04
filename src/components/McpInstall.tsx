import { useState } from "react";
import { Dropdown } from "./Dropdown";
import { Field } from "./EntryField";
import {
  EMPTY_SERVER,
  keyClash,
  pastedServers,
  previousEntry,
  readMcpServer,
  serverKey,
  writeMcpServer,
  type McpServerFields,
  type PastedServer,
} from "../lib/configEntries";
import { isObject, parseFile } from "../lib/configSnippets";
import "./McpInstall.css";

// Installing a server, or editing one: a snippet pasted from a README fills the
// fields, the steps go across — how it runs, how the model reaches its tools,
// the entry itself — and the last shows what goes into mcp.json. What is
// written is the file the JSON editor edits: a way in, not a second copy.

const TRANSPORTS: Record<McpServerFields["transport"], string> = { command: "A command", url: "At a URL" };

type Option<T extends string> = { value: T; title: string; text: string };

// In the order the step offers them: the one a new server starts on first.
const EXPOSURE_OPTIONS: Option<McpServerFields["exposure"]>[] = [
  {
    value: "deferred",
    title: "Found with toolSearch",
    text: "Only the server's name goes in every request. When the model needs one of its tools, it searches for it, calls what the search finds, and keeps it for the rest of the conversation. A server of many tools costs almost nothing until one is used — the default for a new server.",
  },
  {
    value: "direct",
    title: "Declared to the model",
    text: "Every tool, with its name and description, goes in every request like the app's own. The model sees them at once and never searches. Costs their place in the context in every round, used or not.",
  },
  {
    value: "hidden",
    title: "Hidden",
    text: "None of them is offered. Single tools can still be switched on in the server's row in the MCP tab — for a server of which only one or two calls should be within reach.",
  },
];

const RESULT_OPTIONS: Option<"cleared" | "kept">[] = [
  {
    value: "cleared",
    title: "Cleared when the context fills",
    text: "Once the request fills most of the model's window, the oldest results are dropped as any tool's are, and the model calls the tool again when it needs them. A long conversation stays inside the window, at the price of a repeated call now and then.",
  },
  {
    value: "kept",
    title: "Kept",
    text: "Every result stays for the whole conversation — for a server that answers with templates, rules or a style guide the model follows, which would otherwise be fetched again every few rounds. Costs the context they take all the way down.",
  },
];

/**
 * A server typed in from scratch starts with its tools found by search: a
 * README's snippet rarely says, and declaring a whole server's tools in every
 * request is the expensive way to begin.
 */
const NEW_SERVER: McpServerFields = { ...EMPTY_SERVER, exposure: "deferred" };

type Step = "paste" | "connection" | "tools" | "review";

// In the order the screen walks them.
const STEPS: { id: Step; label: string }[] = [
  { id: "paste", label: "Paste" },
  { id: "connection", label: "Connection" },
  { id: "tools", label: "Tools" },
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

/** One way of doing something, and the reason it is worth taking. */
function Choice<T extends string>({
  label,
  value,
  options,
  onPick,
}: {
  label: string;
  value: T;
  options: Option<T>[];
  onPick: (value: T) => void;
}) {
  return (
    <div className="mcp-install-choices" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={`mcp-install-choice${option.value === value ? " active" : ""}`}
          onClick={() => onPick(option.value)}
        >
          <span className="mcp-install-choice-title">{option.title}</span>
          <span className="mcp-install-choice-text">{option.text}</span>
        </button>
      ))}
    </div>
  );
}

export function McpInstall({ name, text, path, error, onSave, onClose, onEditJson }: Props) {
  const [step, setStep] = useState<Step>(name === null ? "paste" : "connection");
  const [fields, setFields] = useState<McpServerFields>(
    () => (name !== null && text ? readMcpServer(text, name) : null) ?? NEW_SERVER,
  );
  const [draft, setDraft] = useState("");
  // What the last paste held, or why it held nothing.
  const [found, setFound] = useState<{ servers: PastedServer[] } | { error: string } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const typed = fields.name.trim();
  // A pasted name that is already in the file replaces that entry where it
  // stands, as the file editor's own paste does.
  const previous = previousEntry(text ?? "", name, typed);
  // What the agent will call this server's tools, and whose prefix it takes
  // away from if another server in the file is spelled differently.
  const key = serverKey(typed);
  const clash = keyClash(text ?? "", previous, typed);
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
    const result = pastedServers(draft, NEW_SERVER.exposure);
    setFound(result);
    if ("servers" in result && result.servers.length === 1) pick(result.servers[0]);
  };

  // Nothing is written over a file that was never read: an unreadable file is
  // empty here, and saving would leave its servers behind.
  const next = () => {
    if (text === undefined) return setProblem(error ?? "The MCP file has not been read yet");
    if ("error" in written) return setProblem(written.error);
    go(step === "connection" ? "tools" : "review");
  };

  const install = async () => {
    if (text === undefined) return setProblem(error ?? "The MCP file has not been read yet");
    if ("error" in written) return setProblem(written.error);
    setSaving(true);
    const stored = await onSave(written.text);
    setSaving(false);
    if (stored) onClose();
  };

  const submit = () => {
    if (step === "paste") {
      if (draft.trim()) read();
      return;
    }
    if (step === "review") return void install();
    next();
  };

  const set = (key: keyof McpServerFields) => (e: { target: { value: string } }) => {
    setProblem(null);
    setFields((f) => ({ ...f, [key]: e.target.value }));
  };

  return (
    <form
      className="mcp-install"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <nav className="mcp-install-steps" aria-label="Steps">
        {steps.map(({ id, label }, i) => (
          <button
            key={id}
            type="button"
            className="mcp-install-step"
            aria-current={step === id ? "step" : undefined}
            title={i > at ? "Fill this step in first" : undefined}
            disabled={i > at}
            onClick={() => go(id)}
          >
            <span className="mcp-install-step-num">{i + 1}</span>
            {label}
          </button>
        ))}
      </nav>

      {/* Any refusal, wherever it came from, stands here until the next move. */}
      {(problem ?? error) && <p className="entry-error">{problem ?? error}</p>}

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
              <Field label="Name" hint="Letters and digits stay, anything else becomes _; the name is lowercased.">
                <input className="entry-input" value={fields.name} onChange={set("name")} autoFocus={name === null} />
                <code className="mcp-install-key">
                  {`mcp__${key || "<name>"}__`}
                  <span className="mcp-install-key-tool">{"<tool>"}</span>
                </code>
              </Field>
              {clash && (
                <p className="mcp-install-warn">
                  Clashes with the server <b>{clash}</b> in the file: both become <b>{key}</b> in tool names — rename
                  one, or its tools will not be offered.
                </p>
              )}
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
            </section>
          </div>
        </>
      )}

      {step === "tools" && (
        <div className="mcp-install-columns">
          <section className="mcp-install-col">
            <h4 className="mcp-install-head">How the model reaches its tools</h4>
            <Choice
              label="How the model reaches its tools"
              value={fields.exposure}
              options={EXPOSURE_OPTIONS}
              onPick={(exposure) => setFields((f) => ({ ...f, exposure }))}
            />
            <p className="mcp-install-note">
              The server's own tools, and a switch on each of them, are in its row in the MCP tab once it starts.
            </p>
          </section>
          <section className="mcp-install-col">
            <h4 className="mcp-install-head">What happens to its results</h4>
            <Choice
              label="What happens to its results as the context fills"
              value={fields.keepResults ? "kept" : "cleared"}
              options={RESULT_OPTIONS}
              onPick={(value) => setFields((f) => ({ ...f, keepResults: value === "kept" }))}
            />
          </section>
        </div>
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
            <button className="btn btn-primary" type="submit" disabled={!draft.trim()}>
              Continue
            </button>
            <button
              className="btn btn-ghost"
              type="button"
              onClick={() => {
                setFields(NEW_SERVER);
                go("connection");
              }}
            >
              Start from scratch
            </button>
          </>
        )}
        {(step === "connection" || step === "tools") && (
          <>
            <button className="btn btn-primary" type="submit">
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
            <button className="btn btn-primary" type="submit" disabled={saving}>
              {name !== null ? "Save" : previous !== null ? "Replace" : "Install"}
            </button>
            <button className="btn btn-ghost" type="button" onClick={() => go("tools")}>
              Back
            </button>
          </>
        )}
        <button className="link-btn mcp-install-json" type="button" onClick={onEditJson}>
          Edit as JSON
        </button>
      </div>
    </form>
  );
}
