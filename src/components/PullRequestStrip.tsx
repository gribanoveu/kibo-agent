import { useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  CircleAlert,
  CircleCheck,
  CircleHelp,
  CircleX,
  ExternalLink,
  GitMerge,
  GitPullRequest,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { Modal } from "./Modal";
import { CodeHostConnect } from "./CodeHostConnect";
import { openExternal } from "../lib/window";
import { buildsAtAGlance, reviewsAtAGlance } from "../lib/pullRequestView";
import type { BuildState, PrStrip, PullRequest, ReviewStatus } from "../lib/pullRequests";
import type { PullRequestState } from "../hooks/usePullRequest";
import "./PullRequestStrip.css";

type Props = PullRequestState & { onNotify: (msg: string) => void };
type Ready = Extract<PrStrip, { state: "ready" }>;

const BUILD_ICON: Record<BuildState, typeof Circle> = {
  successful: CircleCheck,
  failed: CircleX,
  inProgress: Loader2,
  unknown: CircleHelp,
};

const REVIEW: Record<ReviewStatus, { icon: typeof Circle; label: string }> = {
  approved: { icon: Check, label: "approved" },
  needsWork: { icon: CircleAlert, label: "needs work" },
  unapproved: { icon: Circle, label: "not yet" },
};

function BuildIcon({ state }: { state: BuildState }) {
  const Icon = BUILD_ICON[state];
  return <Icon size={12} className={`pr-build ${state}`} aria-label={`build ${state}`} />;
}

/**
 * The pull request of the branch checked out, above the Changes list
 * (docs/56-bitbucket-prs.md, decision 5): one line, opened into its reviewers,
 * builds and buttons. A folder with no remote or no branch shows nothing; a
 * remote no server answers to offers to connect one; a read that failed keeps
 * the last strip, grey, with the reason.
 */
export function PullRequestStrip(props: Props) {
  const { strip, error, reading, refresh, onNotify } = props;
  const [connecting, setConnecting] = useState(false);

  if (!strip) {
    return error ? (
      <div className="pr-strip stale">
        <div className="pr-line">
          <GitPullRequest size={13} className="pr-icon" />
          <span className="pr-title pr-reason" title={error}>
            {error}
          </span>
          <RefreshButton reading={reading} onRefresh={refresh} />
        </div>
      </div>
    ) : null;
  }
  if (strip.state === "noRemote" || strip.state === "noBranch") return null;

  if (strip.state === "notConnected") {
    const { remote } = strip;
    return (
      <div className="pr-strip">
        <div className="pr-line">
          <GitPullRequest size={13} className="pr-icon" />
          <span className="pr-title pr-muted">{remote.host} is not connected</span>
          <button type="button" className="btn btn-ghost pr-btn" onClick={() => setConnecting(true)}>
            Connect…
          </button>
        </div>
        <CodeHostConnect
          open={connecting}
          server={{ id: "", baseUrl: remote.baseUrl, hosts: [], trustedCertPem: null, tokenSet: false }}
          forFolder
          onClose={() => setConnecting(false)}
          onSave={props.connect}
          onDetect={props.detect}
        />
      </div>
    );
  }

  return <ReadyStrip {...props} strip={strip} error={error} onNotify={onNotify} />;
}

function RefreshButton({ reading, onRefresh }: { reading: boolean; onRefresh: () => void }) {
  return (
    <button type="button" className="pr-icon-btn" title="Read again" aria-label="Refresh pull request" onClick={onRefresh}>
      <RefreshCw size={12} className={reading ? "pr-spin" : undefined} />
    </button>
  );
}

function ReadyStrip({ strip, error, reading, refresh, create, approve, merge, onNotify }: Props & { strip: Ready }) {
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState<"merge" | "create" | null>(null);
  const [busy, setBusy] = useState(false);
  const { snapshot, branch } = strip;
  const pr = snapshot.pr;

  const run = async (action: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await action();
      if (done) onNotify(done);
      setConfirm(null);
    } catch (e) {
      onNotify(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`pr-strip${error ? " stale" : ""}`}>
      {pr ? (
        <button type="button" className="pr-line pr-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          <GitPullRequest size={13} className="pr-icon" />
          <span className="pr-title" title={pr.title}>
            <span className="pr-number">#{pr.id}</span> {pr.title}
          </span>
          <Glance pr={pr} builds={snapshot.builds} />
        </button>
      ) : (
        <div className="pr-line">
          <GitPullRequest size={13} className="pr-icon" />
          <span className="pr-title pr-muted">{snapshot.branchOnServer ? `No pull request for ${branch}` : "Push the branch first"}</span>
          <button
            type="button"
            className="btn btn-ghost pr-btn"
            disabled={!snapshot.branchOnServer || busy}
            title={snapshot.branchOnServer ? undefined : `${branch} is not on the server yet`}
            onClick={() => setConfirm("create")}
          >
            Create PR
          </button>
          <RefreshButton reading={reading} onRefresh={refresh} />
        </div>
      )}
      {error && <p className="pr-reason">{error}</p>}

      {pr && open && (
        <div className="pr-details">
          <div className="pr-meta">
            {pr.author} · {pr.fromBranch} → {pr.toBranch}
          </div>
          {pr.reviewers.length > 0 && (
            <ul className="pr-list" aria-label="Reviewers">
              {pr.reviewers.map((r) => {
                const { icon: Icon, label } = REVIEW[r.status];
                return (
                  <li key={r.slug} className={`pr-reviewer ${r.status}`}>
                    <Icon size={12} />
                    <span>{r.slug}</span>
                    <span className="pr-muted">{label}</span>
                  </li>
                );
              })}
            </ul>
          )}
          {snapshot.builds.length > 0 && (
            <ul className="pr-list" aria-label="Builds">
              {snapshot.builds.map((b) => (
                <li key={b.key}>
                  <button type="button" className="pr-link" title={b.url} onClick={() => void openExternal(b.url)}>
                    <BuildIcon state={b.state} />
                    <span>{b.name}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {strip.mergeBlocked && (
            <ul className="pr-list pr-vetoes" aria-label="Why Merge is off">
              {strip.mergeBlocked.map((veto) => (
                <li key={veto}>{veto}</li>
              ))}
            </ul>
          )}
          <div className="pr-actions">
            <button type="button" className="btn btn-ghost pr-btn" onClick={() => void openExternal(pr.url)}>
              <ExternalLink size={12} /> Open
            </button>
            {(strip.canApprove || strip.approved) && (
              <button type="button" className="btn btn-ghost pr-btn" disabled={busy} onClick={() => void run(() => approve(!strip.approved))}>
                {strip.approved ? "Unapprove" : "Approve"}
              </button>
            )}
            <button
              type="button"
              className="btn btn-primary pr-btn"
              disabled={strip.mergeBlocked !== null || busy}
              onClick={() => setConfirm("merge")}
            >
              <GitMerge size={12} /> Merge
            </button>
            <RefreshButton reading={reading} onRefresh={refresh} />
          </div>
        </div>
      )}

      {pr && (
        <Modal
          title="Merge pull request"
          open={confirm === "merge"}
          onClose={() => setConfirm(null)}
          footer={
            <>
              <button type="button" className="btn btn-ghost" onClick={() => setConfirm(null)}>
                Cancel
              </button>
              <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void run(() => merge(pr.id, pr.version), `Merged #${pr.id}`)}>
                Merge
              </button>
            </>
          }
        >
          <p className="pr-confirm">
            Merge <b>#{pr.id} {pr.title}</b> from {pr.fromBranch} into <b>{pr.toBranch}</b> on the server? This cannot be undone
            from here.
          </p>
        </Modal>
      )}
      {!pr && (
        <CreateDialog
          open={confirm === "create"}
          branch={branch}
          target={snapshot.defaultBranch ?? ""}
          busy={busy}
          onClose={() => setConfirm(null)}
          onCreate={(title, to) => void run(() => create(title, to), "Pull request created")}
        />
      )}
    </div>
  );
}

/** The collapsed line's right side: the builds as one icon, the reviews as a count. */
function Glance({ pr, builds }: { pr: PullRequest; builds: Ready["snapshot"]["builds"] }) {
  const build = buildsAtAGlance(builds);
  const reviews = reviewsAtAGlance(pr.reviewers);
  return (
    <span className="pr-glance">
      {build && <BuildIcon state={build} />}
      {reviews && <span className="pr-muted">{reviews}</span>}
    </span>
  );
}

function CreateDialog({
  open,
  branch,
  target,
  busy,
  onClose,
  onCreate,
}: {
  open: boolean;
  branch: string;
  target: string;
  busy: boolean;
  onClose: () => void;
  onCreate: (title: string, to: string | null) => void;
}) {
  const [title, setTitle] = useState("");
  const [to, setTo] = useState(target);
  return (
    <Modal
      title="Create pull request"
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => onCreate(title, to.trim() || null)}>
            Create
          </button>
        </>
      }
    >
      <div className="modal-field">
        <label htmlFor="pr-title">Title</label>
        <input id="pr-title" value={title} placeholder={branch} onChange={(e) => setTitle(e.target.value)} />
      </div>
      <div className="modal-field">
        <label htmlFor="pr-target">Into</label>
        <input id="pr-target" value={to} placeholder={target} spellCheck={false} onChange={(e) => setTo(e.target.value)} />
      </div>
      <p className="modal-note">
        From {branch}. The pull request is opened on the server, where it stays until it is declined there.
      </p>
    </Modal>
  );
}
