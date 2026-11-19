import { useState } from "react";
import { Loader2, Minus, Plus, Sparkles } from "lucide-react";
import { useStaging } from "../hooks/useStaging";
import type { ChangedFile, FileTarget } from "../lib/git";
import { useGitHistory } from "../hooks/useGitHistory";
import { HistoryView } from "./HistoryView";
import { Tabs } from "./Tabs";
import "./ChangesPanel.css";

type Props = {
  /** On screen: the status is read only while it is. */
  active: boolean;
  /** The open folder, as the backend's events name it. */
  workspace: string | null;
  onNotify: (msg: string) => void;
  /** The file the viewer shows: its row is marked. */
  openFile: FileTarget | null;
  onOpenFile: (target: FileTarget, pin?: boolean) => void;
  /** The commit message, held above the panel: it outlives the panel being closed or moved. */
  message: string;
  onMessage: (message: string) => void;
};

function StageRow({
  file,
  staged,
  open,
  onToggle,
  onShowDiff,
}: {
  file: ChangedFile;
  staged: boolean;
  open: boolean;
  onToggle: () => void;
  /** `pin` on a double click: the tab is kept rather than reused. */
  onShowDiff: (pin: boolean) => void;
}) {
  // The name leads; the folder from the repository root sits under it.
  const cut = file.path.lastIndexOf("/");
  const name = file.path.slice(cut + 1);
  const dir = cut > 0 ? file.path.slice(0, cut) : null;
  return (
    <div className={`stage-file${open ? " open" : ""}`} aria-current={open || undefined}>
      <button
        type="button"
        className={`stage-btn${staged ? " unstage" : ""}`}
        title={staged ? "Unstage" : "Stage"}
        onClick={onToggle}
      >
        {staged ? <Minus size={12} /> : <Plus size={12} />}
      </button>
      <span
        className="stage-label"
        title={`${file.path} — show diff`}
        onClick={() => onShowDiff(false)}
        onDoubleClick={() => onShowDiff(true)}
      >
        <span className="stage-name">{name}</span>
        {dir && (
          <span className="stage-dir">
            <bdi>{dir}</bdi>
          </span>
        )}
      </span>
      <span className="stat">
        {file.add > 0 && <span className="add">+{file.add}</span>}
        {file.del > 0 && <span className="del">-{file.del}</span>}
      </span>
    </div>
  );
}

export function ChangesPanel({ active, workspace, onNotify, openFile, onOpenFile, message, onMessage }: Props) {
  const [view, setView] = useState<"changes" | "history">("changes");
  const { unstaged, staged, error, stage, unstage, commit, describe } = useStaging(active, workspace);
  const history = useGitHistory(active && view === "history", workspace);
  const [committing, setCommitting] = useState(false);
  const [generating, setGenerating] = useState(false);

  const canCommit = staged.length > 0 && message.trim().length > 0 && !committing;
  // A failed action is said once, in a toast; the lists are read back either way.
  const run = (op: Promise<unknown>) => op.catch((e) => onNotify(String(e)));
  const commitNow = async () => {
    setCommitting(true);
    try {
      const id = await commit(message);
      onMessage("");
      onNotify(`Committed ${id}`);
    } catch (e) {
      onNotify(String(e));
    } finally {
      setCommitting(false);
    }
  };

  const generate = async () => {
    setGenerating(true);
    try {
      onMessage(await describe(message));
    } catch (e) {
      onNotify(String(e));
    } finally {
      setGenerating(false);
    }
  };

  return (
    <div className="changes-panel">
      <Tabs
        label="Changes"
        value={view}
        onChange={setView}
        tabs={[
          { id: "changes", label: "Changes", count: unstaged.length + staged.length },
          { id: "history", label: "History" },
        ]}
      />
      {view === "history" ? (
        <HistoryView {...history} onLoadMore={history.loadMore} />
      ) : error ? (
        <div className="stage-empty">{error}</div>
      ) : (
        <>
          <div className="panel-section stage-section">
            <div className="section-label">
              <span>Changes</span>
              {unstaged.length > 0 && (
                <button className="link-btn" type="button" onClick={() => run(stage(unstaged.map((f) => f.path)))}>
                  Stage all
                </button>
              )}
            </div>
            <div className="stage-list">
              {unstaged.map((f) => (
                <StageRow
                  key={f.path}
                  file={f}
                  staged={false}
                  open={openFile?.side === "unstaged" && openFile.path === f.path}
                  onToggle={() => run(stage([f.path]))}
                  onShowDiff={(pin) => onOpenFile({ path: f.path, side: "unstaged" }, pin)}
                />
              ))}
              {unstaged.length === 0 && <div className="stage-empty">No unstaged changes</div>}
            </div>
          </div>

          <div className="panel-section stage-section">
            <div className="section-label">
              <span>Staged</span>
              <span className="count">{staged.length}</span>
            </div>
            <div className="stage-list">
              {staged.map((f) => (
                <StageRow
                  key={f.path}
                  file={f}
                  staged
                  open={openFile?.side === "staged" && openFile.path === f.path}
                  onToggle={() => run(unstage([f.path]))}
                  onShowDiff={(pin) => onOpenFile({ path: f.path, side: "staged" }, pin)}
                />
              ))}
              {staged.length === 0 && <div className="stage-empty">Stage files to commit</div>}
            </div>
          </div>

          <div className="panel-section commit-section">
            <div className="section-label">
              <span>Commit message</span>
            </div>
            <textarea
              className="commit-msg"
              rows={4}
              placeholder="Describe the commit…"
              value={message}
              onChange={(e) => onMessage(e.target.value)}
            />
            <div className="commit-actions">
              <button
                className="btn btn-ghost"
                type="button"
                disabled={staged.length === 0 || generating}
                onClick={generate}
              >
                {generating ? <Loader2 className="commit-gen-spin" size={13} /> : <Sparkles size={13} />}
                {generating ? "Generating…" : "Generate description"}
              </button>
              <button className="btn btn-primary" type="button" disabled={!canCommit} onClick={commitNow}>
                Commit
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
