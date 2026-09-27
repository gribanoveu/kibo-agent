import { useEffect, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { AgentList } from "./AgentList";
import { ProcessList } from "./ProcessList";
import { useAgents } from "../hooks/useAgents";
import { useProcesses } from "../hooks/useProcesses";
import { useTerminals } from "../hooks/useTerminals";
import { useTerminalScreen } from "../hooks/useTerminalScreen";
import { queuePaste } from "../lib/pasteAtPrompt";
import { terminalQuote, terminalTitle, type TerminalInfo } from "../lib/terminal";
import "./TerminalPanel.css";

type Props = {
  active: boolean;
  workspace: string | null;
  /** The background process a chat row asked to see; a new object each ask. */
  processFocus: { id: number } | null;
  /** A command an answer asked to put in a shell; a new object each ask. */
  terminalPaste: { command: string } | null;
  /** Says the ask was taken, so it is not taken again when the pane is drawn anew. */
  onTerminalPasted: () => void;
  /** Puts text into the message being written. */
  onAddToChat: (text: string) => void;
};

/** What the pane shows: one of the user's shells, the agent's background processes, or its helpers. */
type Shown = number | "processes" | "agents";

/** One shell's screen, and "Add to chat" over it while something is selected. */
function TerminalView({ terminal, onAddToChat }: { terminal: TerminalInfo; onAddToChat: (text: string) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState("");
  const { clearSelection } = useTerminalScreen(terminal.id, ref, setSelection);
  return (
    <div className="terminal-screen">
      <div ref={ref} className="terminal-view" />
      {selection.trim() && (
        <button
          type="button"
          className="terminal-quote"
          onClick={() => {
            onAddToChat(terminalQuote(terminal, selection));
            clearSelection();
            setSelection("");
          }}
        >
          Add to chat
        </button>
      )}
    </div>
  );
}

/**
 * The Terminal tab: the user's own shells, a tab each, and the agent's
 * background processes and helper agents as the last two tabs. Opening the pane gives a shell, as a
 * terminal window does — once per folder, not again after the last is closed.
 */
export function TerminalPanel({ active, workspace, processFocus, terminalPaste, onTerminalPasted, onAddToChat }: Props) {
  const { processes, error: processError, stop } = useProcesses(active);
  const { agents, error: agentError, stop: stopAgent } = useAgents(active);
  const shells = useTerminals(active);
  const [picked, setPicked] = useState<Shown | null>(null);
  const openedFor = useRef<string | null>(null);
  // The ask taken from App. App lets go of it once told, but StrictMode runs
  // this effect a second time before App has heard.
  const taken = useRef<object | null>(null);

  useEffect(() => {
    if (!active || !workspace || !shells.loaded) return;
    const first = openedFor.current !== workspace;
    openedFor.current = workspace;
    const ask = terminalPaste && taken.current !== terminalPaste ? terminalPaste : null;
    if (!ask) {
      if (first && shells.terminals.length === 0) void shells.open().then((t) => t && setPicked(t.id));
      return;
    }
    taken.current = ask;
    onTerminalPasted();
    // The shell on screen while it still runs, else the newest that does, else a new one.
    const live = shells.terminals.filter((t) => t.state.state === "running");
    const target = live.find((t) => t.id === picked) ?? live[live.length - 1];
    // Queued by the terminal's id: whichever screen draws it pastes it.
    const show = (id: number) => {
      queuePaste(id, ask.command);
      setPicked(id);
    };
    if (target) show(target.id);
    else void shells.open().then((t) => t && show(t.id));
  }, [active, workspace, shells.loaded, shells.terminals, shells.open, terminalPaste, onTerminalPasted, picked]);

  useEffect(() => {
    if (processFocus) setPicked("processes");
  }, [processFocus]);

  // A picked shell that was closed falls back to the newest one left.
  const newest = shells.terminals[shells.terminals.length - 1];
  const listed = picked === "processes" || picked === "agents" ? picked : null;
  const drawn = listed ? undefined : (shells.terminals.find((t) => t.id === picked) ?? newest);
  const shown: Shown = drawn?.id ?? listed ?? "processes";
  const running = processes.filter((p) => p.state.state === "running").length;
  const helping = agents.filter((a) => a.state.state === "running").length;
  const newShell = () => void shells.open().then((t) => t && setPicked(t.id));

  return (
    <div className="terminal-panel">
      <div className="terminal-tabs" role="tablist" aria-label="Terminals">
        {shells.terminals.map((t) => (
          <div key={t.id} className={`terminal-tab${shown === t.id ? " selected" : ""}`}>
            <button
              type="button"
              role="tab"
              aria-selected={shown === t.id}
              className={`terminal-tab-name${t.state.state === "running" ? "" : " ended"}`}
              onClick={() => setPicked(t.id)}
            >
              {terminalTitle(t)}
            </button>
            <button
              type="button"
              className="terminal-tab-close"
              aria-label={`Close ${t.shell}`}
              title="Close — ends what runs in it"
              onClick={() => void shells.close(t.id)}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <button
          type="button"
          className="iconbtn terminal-new"
          title={workspace ? "New terminal" : "Open a folder first"}
          aria-label="New terminal"
          disabled={!workspace}
          onClick={newShell}
        >
          <Plus size={14} />
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={shown === "processes"}
          className={`terminal-tab-name terminal-processes-tab${shown === "processes" ? " selected" : ""}`}
          onClick={() => setPicked("processes")}
        >
          Processes{running > 0 && <span className="terminal-running">{running}</span>}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={shown === "agents"}
          className={`terminal-tab-name terminal-processes-tab${shown === "agents" ? " selected" : ""}`}
          onClick={() => setPicked("agents")}
        >
          Agents{helping > 0 && <span className="terminal-running">{helping}</span>}
        </button>
      </div>
      {shells.error && <div className="terminal-error">{shells.error}</div>}
      {drawn ? (
        <TerminalView key={drawn.id} terminal={drawn} onAddToChat={onAddToChat} />
      ) : shown === "agents" ? (
        <div className="terminal-processes">
          <AgentList agents={agents} error={agentError} onStop={stopAgent} />
        </div>
      ) : (
        <div className="terminal-processes">
          <ProcessList processes={processes} error={processError} onStop={stop} focus={processFocus} />
        </div>
      )}
    </div>
  );
}
