import { useState, type ReactNode } from "react";
import {
  Check,
  Circle,
  CircleAlert,
  CircleCheck,
  CircleHelp,
  CircleX,
  ExternalLink,
  GitMerge,
  GitPullRequest,
  GitPullRequestCreate,
  Hammer,
  Loader2,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";
import { Modal } from "./Modal";
import { CodeHostConnect } from "./CodeHostConnect";
import { openExternal } from "../lib/window";
import { buildText, offersConnect, reviewsAtAGlance } from "../lib/pullRequestView";
import type { Build, BuildState, PrStrip, ReviewStatus } from "../lib/pullRequests";
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

const REVIEW_ICON: Record<ReviewStatus, typeof Circle> = {
  approved: Check,
  needsWork: CircleAlert,
  unapproved: Circle,
};

function BuildIcon({ state }: { state: BuildState }) {
  const Icon = BUILD_ICON[state];
  return <Icon size={12} className={`pr-build ${state}`} aria-label={`build ${state}`} />;
}

/**
 * The branch's pull request, under the branch in Overview's Changes card
 * (docs/56-bitbucket-prs.md, decision 5): one row, opened into its reviewers,
 * builds and what the server lets be done. A button shows only when it can do
 * something — Merge while the server allows it, Approve while the token's
 * user may, Create once the branch is pushed and has none — and a folder that
 * has nothing to show shows nothing. A read that failed keeps the last row,
 * grey, with the reason and a way to read again.
 */
export function PullRequestStrip(props: Props) {
  const { strip, error, reading, refresh } = props;
  const [connecting, setConnecting] = useState(false);

  const failed = error && (
    <div className="pr-failed">
      <TriangleAlert size={12} />
      <span className="pr-failed-text" title={error}>
        {error}
      </span>
      <button type="button" className="pr-icon-btn" title="Read again" aria-label="Refresh pull request" onClick={refresh}>
        <RefreshCw size={12} className={reading ? "pr-spin" : undefined} />
      </button>
    </div>
  );

  // A server out of reach (off the VPN) leaves the branch alone, with no complaint under it.
  if (!strip || strip.state === "noRemote" || strip.state === "noBranch" || strip.state === "unreachable") return failed || null;

  if (strip.state === "notConnected") {
    const { remote } = strip;
    // A GitHub or GitLab folder has no Bitbucket to connect.
    if (!offersConnect(remote.host)) return failed || null;
    return (
      <>
        <button type="button" className="pr-row pr-quiet" title={`No Bitbucket server added for ${remote.host}`} onClick={() => setConnecting(true)}>
          <GitPullRequest size={14} />
          <span className="pr-name">Connect Bitbucket…</span>
        </button>
        <CodeHostConnect
          open={connecting}
          server={{ id: "", baseUrl: remote.baseUrl, hosts: [], trustedCertPem: null, tokenSet: false }}
          forFolder
          onClose={() => setConnecting(false)}
          onSave={props.connect}
          onDetect={props.detect}
        />
        {failed}
      </>
    );
  }

  return <ReadyRow {...props} strip={strip} failed={failed} />;
}

function ReadyRow({ strip, error, create, approve, merge, onNotify, failed }: Props & { strip: Ready; failed: ReactNode }) {
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState<"merge" | "create" | null>(null);
  const [busy, setBusy] = useState(false);
  const { snapshot, branch } = strip;
  const pr = snapshot.pr;
  const build = strip.latestBuild && <BuildRow build={strip.latestBuild} />;

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

  // No pull request: Create, once there is something on the server to open it from.
  if (!pr) {
    if (!snapshot.branchOnServer) return failed || null;
    return (
      <div className={error ? "pr-stale" : undefined}>
        {build}
        <button type="button" className="pr-row pr-quiet" disabled={busy} onClick={() => setConfirm("create")}>
          <GitPullRequestCreate size={14} />
          <span className="pr-name">Create pull request…</span>
        </button>
        {failed}
        <CreateDialog
          open={confirm === "create"}
          branch={branch}
          target={snapshot.defaultBranch ?? ""}
          busy={busy}
          onClose={() => setConfirm(null)}
          onCreate={(title, to) => void run(() => create(title, to), "Pull request created")}
        />
      </div>
    );
  }

  const reviews = reviewsAtAGlance(pr.reviewers);
  const canMerge = strip.mergeBlocked === null;
  const blocked = strip.mergeBlocked ?? [];
  const mayApprove = strip.canApprove || strip.approved;
  return (
    <div className={error ? "pr-stale" : undefined}>
      {build}
      <button type="button" className="pr-row" aria-expanded={open} title={pr.title} onClick={() => setOpen(!open)}>
        <GitPullRequest size={14} />
        <span className="pr-name">
          <span className="pr-number">#{pr.id}</span> {pr.title}
        </span>
        {reviews && (
          <span className="pr-glance" title={`${pr.reviewers.filter((r) => r.status === "approved").length} of ${pr.reviewers.length} reviewers approved`}>
            {reviews}
          </span>
        )}
      </button>
      {failed}

      {open && (
        <div className="pr-more">
          <div className="pr-meta">
            <span title={pr.author}>{pr.authorName}</span> → {pr.toBranch}
          </div>
          {pr.reviewers.length > 0 && (
            <ul className="pr-reviewers" aria-label="Reviewers">
              {pr.reviewers.map((r) => {
                const Icon = REVIEW_ICON[r.status];
                return (
                  <li key={r.slug} className={r.status} title={`${r.slug} — ${r.status === "needsWork" ? "needs work" : r.status}`}>
                    <Icon size={11} />
                    {r.displayName}
                  </li>
                );
              })}
            </ul>
          )}
          {blocked.length > 0 && (
            <ul className="pr-vetoes" aria-label="Why it cannot be merged">
              {blocked.map((veto) => (
                <li key={veto}>{veto}</li>
              ))}
            </ul>
          )}
          <div className="pr-actions">
            <button type="button" className="pr-action" onClick={() => void openExternal(pr.url)}>
              <ExternalLink size={12} /> Open
            </button>
            {mayApprove && (
              <button type="button" className="pr-action" disabled={busy} onClick={() => void run(() => approve(!strip.approved))}>
                <Check size={12} /> {strip.approved ? "Unapprove" : "Approve"}
              </button>
            )}
            {canMerge && (
              <button type="button" className="pr-action pr-primary" disabled={busy} onClick={() => setConfirm("merge")}>
                <GitMerge size={12} /> Merge
              </button>
            )}
          </div>
        </div>
      )}

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
    </div>
  );
}

/** The latest build of the branch's commit: what it is doing, its page a click away. */
function BuildRow({ build }: { build: Build }) {
  return (
    <button
      type="button"
      className="pr-row pr-quiet"
      title={build.url ? `${build.name} — open` : build.name}
      disabled={!build.url}
      onClick={() => void openExternal(build.url)}
    >
      <Hammer size={14} />
      <span className="pr-name">{buildText(build.state)}</span>
      <BuildIcon state={build.state} />
    </button>
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
