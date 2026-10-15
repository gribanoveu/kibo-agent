import type { CommitSummary, GitHistory } from "../lib/git";
import "./HistoryView.css";

/** How long ago, as a person says it: "now", "55m ago", "yesterday", "3d ago", then the date. */
export function ago(seconds: number, now = Date.now() / 1000) {
  const s = Math.max(0, now - seconds);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 2 * 86400) return "yesterday";
  if (s < 7 * 86400) return `${Math.floor(s / 86400)}d ago`;
  return new Date(seconds * 1000).toLocaleDateString();
}

function CommitRow({ commit }: { commit: CommitSummary }) {
  return (
    <li className={`history-commit${commit.head ? " head" : ""}`}>
      <span className="history-dot" aria-hidden="true" />
      <div className="history-body">
        <div className="history-summary" title={commit.summary}>
          {commit.summary}
        </div>
        <div className="history-meta" title={new Date(commit.time * 1000).toLocaleString()}>
          {commit.head && <span className="history-ref head">HEAD</span>}
          {commit.refs.map((name) => (
            <span key={name} className="history-ref">
              {name}
            </span>
          ))}
          <span className="history-id">{commit.id}</span>
          <span className="history-sep">·</span>
          <span className="history-author">{commit.author}</span>
          <span className="history-sep">·</span>
          <span>{ago(commit.time)}</span>
        </div>
      </div>
    </li>
  );
}

/** The Changes panel's History tab: the branch against its upstream, then the commits as a line. */
export function HistoryView({
  history,
  error,
  onLoadMore,
}: {
  history: GitHistory | null;
  error: string | null;
  onLoadMore: () => void;
}) {
  if (error) return <div className="history-empty">{error}</div>;
  if (!history) return null;
  const { branch, upstream, ahead, behind, commits, more } = history;
  return (
    <div className="history">
      <div className="section-label">
        <span>Git history</span>
        {more && (
          <button type="button" className="link-btn" onClick={onLoadMore}>
            Load more
          </button>
        )}
      </div>
      {branch && (
        <div className="history-branch">
          <span className="history-branch-name">{branch}</span>
          {ahead > 0 && <span className="add" title={`${ahead} not pushed`}>↑{ahead}</span>}
          {behind > 0 && <span className="del" title={`${behind} not pulled`}>↓{behind}</span>}
          {upstream && <span className="history-upstream">{upstream}</span>}
        </div>
      )}
      {commits.length === 0 ? (
        <div className="history-empty">No commits yet</div>
      ) : (
        <ol className="history-list">
          {commits.map((c) => (
            <CommitRow key={c.id} commit={c} />
          ))}
        </ol>
      )}
    </div>
  );
}
