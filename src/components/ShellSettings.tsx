import { useShellChoice } from "../hooks/useShellChoice";
import type { ShellKind } from "../lib/chat";
import "./Segmented.css";

const LABELS: Record<ShellKind, string> = {
  cmd: "cmd",
  windowsPowershell: "PowerShell 5",
  powershell: "PowerShell 7",
  gitBash: "Git Bash",
};

/**
 * The shell the agent's commands and the terminal tab run in. Only Windows has a choice; elsewhere
 * nothing is drawn — commands run under `/bin/sh` and the terminal under the login shell.
 */
export function ShellSettings() {
  const { choices, error, choose } = useShellChoice();
  // Read once the pane opens; nothing to pick, or nothing read, is nothing drawn.
  if (!choices?.options.length) return null;
  const chosen = choices.options.find((o) => o.kind === choices.chosen);
  const bash = choices.options.some((o) => o.kind === "gitBash");

  return (
    <>
      <div className="modal-field">
        <label>Shell</label>
        <div className="segmented" role="radiogroup" aria-label="Shell">
          {choices.options.map(({ kind }) => (
            <button
              key={kind}
              type="button"
              role="radio"
              aria-checked={choices.chosen === kind}
              className={`segment${choices.chosen === kind ? " active" : ""}`}
              onClick={() => void choose(kind)}
            >
              {LABELS[kind]}
            </button>
          ))}
        </div>
      </div>
      <p className="modal-note settings-hint">
        Runs the agent's commands and opens in the terminal tab, from the next of each on.
        {chosen && ` ${chosen.program}`}
        {bash && " Models write commands for bash best: Git Bash fails fewest of them."}
      </p>
      {error && <p className="modal-note settings-hint settings-error">{error}</p>}
    </>
  );
}
