import { useState } from "react";
import { Dropdown } from "./Dropdown";
import { LABELS, toolLabel } from "../lib/describeTool";
import type { CallStatus, ToolLogFilter, ToolLogRow } from "../lib/chat";
import "./ToolLog.css";
import "./Segmented.css";

type Props = {
  rows: ToolLogRow[];
  total: number;
  filter: ToolLogFilter;
  onFilter: (filter: ToolLogFilter) => void;
  onMore: () => void;
  onClear: () => void;
  enabled: boolean;
  onToggle: (enabled: boolean) => void;
  error: string | null;
};

const STATUSES: { value: string; label: string }[] = [
  { value: "", label: "All outcomes" },
  { value: "ok", label: "ok" },
  { value: "error", label: "error" },
  { value: "denied", label: "denied" },
];

const TONE: Record<CallStatus, string> = { ok: "ok", error: "bad", denied: "warn" };

/** What the call was about, in one line: the path, the command, the query or the skill. */
function target(row: ToolLogRow): string {
  const args = row.args?.args ?? {};
  for (const key of ["path", "command", "query", "pattern", "name"]) {
    if (typeof args[key] === "string") return args[key] as string;
  }
  return row.args === null ? "(arguments did not parse)" : "";
}

function time(tsMs: number): string {
  const at = new Date(tsMs);
  const clock = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  return at.toDateString() === new Date().toDateString() ? clock : `${at.toLocaleDateString()} ${clock}`;
}

function Row({ row }: { row: ToolLogRow }) {
  const [open, setOpen] = useState(false);
  return (
    <li className={`log-row${open ? " open" : ""}`}>
      <button type="button" className="log-head" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <span className="log-time">{time(row.tsMs)}</span>
        <span className="log-tool">{toolLabel(row.tool)}</span>
        <span className="log-target">{target(row)}</span>
        <span className={`log-status ${TONE[row.status]}`}>{row.status}</span>
        <span className="log-ms">{row.durationMs} ms</span>
      </button>
      {open && (
        <div className="log-detail">
          <div className="log-meta">
            {row.providerId} · {row.model} · round {row.round} · {row.repoRoot}
          </div>
          {row.error && <div className="log-error">{row.error}</div>}
          <pre>{JSON.stringify(row.args, null, 2)}</pre>
          {row.result !== null && row.result !== undefined && <pre>{JSON.stringify(row.result, null, 2)}</pre>}
        </div>
      )}
    </li>
  );
}

/** The log window's body: filters, the rows, and the switch. Never shows file content — the log holds none. */
export function ToolLog({ rows, total, filter, onFilter, onMore, onClear, enabled, onToggle, error }: Props) {
  // Emptying is irreversible, so it takes a second press.
  const [confirming, setConfirming] = useState(false);
  const tools = [{ value: "", label: "All tools" }, ...Object.keys(LABELS).map((t) => ({ value: t, label: LABELS[t] }))];

  return (
    <div className="tool-log">
      <div className="log-filters">
        <Dropdown
          label={filter.tool ? toolLabel(filter.tool) : "All tools"}
          options={tools}
          value={filter.tool ?? ""}
          onPick={(tool) => onFilter({ ...filter, tool: tool || undefined })}
        />
        <Dropdown
          label={filter.status ?? "All outcomes"}
          options={STATUSES}
          value={filter.status ?? ""}
          onPick={(status) => onFilter({ ...filter, status: (status || undefined) as CallStatus | undefined })}
        />
        <input
          className="log-search"
          type="search"
          placeholder="Search paths, commands, errors"
          aria-label="Search the log"
          value={filter.search ?? ""}
          onChange={(e) => onFilter({ ...filter, search: e.target.value || undefined })}
        />
      </div>

      {error && <div className="log-error">{error}</div>}
      {rows.length === 0 ? (
        <div className="empty">{enabled ? "No tool calls match." : "The log is off: new calls are not recorded."}</div>
      ) : (
        <ul className="log-rows">
          {rows.map((row) => (
            <Row key={row.id} row={row} />
          ))}
        </ul>
      )}

      <div className="log-foot">
        <span className="log-count">
          {rows.length} of {total}
        </span>
        {rows.length < total && (
          <button type="button" className="btn btn-ghost" onClick={onMore}>
            Load more
          </button>
        )}
        <div className="segmented" role="radiogroup" aria-label="Record tool calls">
          {[
            ["recording", true],
            ["off", false],
          ].map(([label, value]) => (
            <button
              key={String(label)}
              type="button"
              role="radio"
              aria-checked={enabled === value}
              className={`segment${enabled === value ? " active" : ""}`}
              onClick={() => onToggle(Boolean(value))}
            >
              {label}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="btn btn-ghost"
          disabled={total === 0}
          onClick={() => {
            if (!confirming) return setConfirming(true);
            setConfirming(false);
            onClear();
          }}
          onBlur={() => setConfirming(false)}
        >
          {confirming ? `Delete all ${total}?` : "Clear log"}
        </button>
      </div>
    </div>
  );
}
