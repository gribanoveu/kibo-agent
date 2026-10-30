import { useEffect, useRef, useState } from "react";
import { ChevronRight, Folder, Square } from "lucide-react";
import { processStatus, type ProcessView } from "../lib/processes";
import "./ProcessList.css";

type Props = {
  processes: ProcessView[];
  error: string | null;
  onStop: (id: number) => void;
  /** A process asked for from the chat: it opens and scrolls into view; `null` asks for the list as it opens. A new object each ask. */
  focus?: { id: number | null } | null;
};

/** Kept scrolled to the end, where a running process's news is. */
function Tail({ text }: { text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [text]);
  return (
    <pre ref={ref} className={`process-tail${text ? "" : " empty"}`}>
      {text || "No output yet."}
    </pre>
  );
}

/** The agent's background processes: what runs, where, how it ended, and the end of what it wrote. */
export function ProcessList({ processes, error, onStop, focus = null }: Props) {
  // The newest is the one being watched; others open on a click.
  const [open, setOpen] = useState<number | null>(null);
  const shown = open ?? processes[0]?.id ?? null;
  const cards = useRef(new Map<number, HTMLDivElement>());

  useEffect(() => {
    if (!focus) return;
    setOpen(focus.id);
    if (focus.id !== null) cards.current.get(focus.id)?.scrollIntoView?.({ block: "nearest" });
  }, [focus]);

  if (!processes.length) {
    return (
      <div className="process-list-empty">
        {error ??
          "No background processes. The agent starts one with runCommand in the background — a dev server, a watcher — and it runs until stopped."}
      </div>
    );
  }
  return (
    <div className="process-list">
      {error && <div className="process-list-error">{error}</div>}
      {processes.map((p) => {
        const running = p.state.state === "running";
        const failed = p.state.state === "exited" && p.state.code !== 0;
        const state = running ? "running" : failed ? "failed" : "ended";
        const isOpen = shown === p.id;
        return (
          <div
            key={p.id}
            ref={(node) => {
              if (node) cards.current.set(p.id, node);
              else cards.current.delete(p.id);
            }}
            className={`process ${state}${isOpen ? " open" : ""}`}
          >
            <div className="process-head" onClick={() => setOpen(isOpen ? -1 : p.id)}>
              <span className="process-dot" aria-hidden />
              <div className="process-body">
                {/* The whole command, wrapped: the card is where it is read in full. */}
                <code className="process-command">{p.command}</code>
                <div className="process-meta">
                  <span className="process-id">#{p.id}</span>
                  <span className="process-cwd" title="Where it runs, from the open folder">
                    <Folder size={11} aria-hidden />
                    {p.cwd === "." ? "project root" : p.cwd}
                  </span>
                  <span className="process-state">{processStatus(p.state)}</span>
                </div>
              </div>
              {running && (
                <button
                  type="button"
                  className="process-stop"
                  title="Stop"
                  onClick={(e) => {
                    e.stopPropagation();
                    onStop(p.id);
                  }}
                >
                  <Square size={10} fill="currentColor" aria-hidden />
                  Stop
                </button>
              )}
              <ChevronRight className="process-chev" size={13} aria-hidden />
            </div>
            {isOpen && <Tail text={p.tail} />}
          </div>
        );
      })}
    </div>
  );
}
