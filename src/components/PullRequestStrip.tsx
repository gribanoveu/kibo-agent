import { useState, type ReactNode } from "react";
import {
  Check,
  Circle,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleHelp,
  CircleX,
  ExternalLink,
  GitMerge,
  GitPullRequest,
  GitPullRequestCreate,
  GitPullRequestDraft,
  Hammer,
  Loader2,
  MoreHorizontal,
  RefreshCw,
  TriangleAlert,
  Undo2,
} from "lucide-react";
import { Modal } from "./Modal";
import { CodeHostConnect } from "./CodeHostConnect";
import { ChatMenu, type ChatMenuItem } from "./ChatMenu";
import { Switch } from "./Switch";
import { openExternal } from "../lib/window";
import { buildText, offersConnect, readiness, REVIEW_WORD, reviewersInOrder, reviewsAtAGlance } from "../lib/pullRequestView";
import type { Build, BuildState, PrStrip, PullRequest, ReviewStatus } from "../lib/pullRequests";
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

/** Whether the pull request can go in: merge, not yet, or a draft. */
const READY_ICON: Record<"ready" | "blocked" | "draft", typeof Circle> = {
  ready: GitMerge,
  blocked: CircleAlert,
  draft: CircleDashed,
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
 * (docs/56-bitbucket-prs.md, decision 5): its title, "repo #id" and the
 * reviews under it, a dot for whether it can go in, the "⋯" of what else can
 * be done, and one button for the next step — Mark ready on a draft, Merge
 * while the server allows it. Clicked, it opens into its reviewers and why it
 * cannot be merged. A button shows only when it can do something — Approve
 * while the token's user may, Create once the branch is pushed, has none and
 * is not the default branch — and a folder that
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

function ReadyRow({ strip, error, create, approve, merge, setDraft, onNotify, failed }: Props & { strip: Ready; failed: ReactNode }) {
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

  // No pull request: Create, once there is something on the server to open it
  // from — and not on the default branch, which pull requests go into.
  if (!pr) {
    if (!snapshot.branchOnServer) return failed || null;
    const onDefault = branch === snapshot.defaultBranch;
    if (onDefault && !build) return failed || null;
    return (
      <div className={error ? "pr-stale" : undefined}>
        {build}
        {!onDefault && (
          <button type="button" className="pr-row pr-quiet" disabled={busy} onClick={() => setConfirm("create")}>
            <GitPullRequestCreate size={14} />
            <span className="pr-name">Create pull request…</span>
          </button>
        )}
        {failed}
        <CreateDialog
          open={confirm === "create"}
          branch={branch}
          target={snapshot.defaultBranch ?? ""}
          busy={busy}
          onClose={() => setConfirm(null)}
          onCreate={(title, to, draft) => void run(() => create(title, to, draft), draft ? "Draft pull request created" : "Pull request created")}
        />
      </div>
    );
  }

  const reviews = reviewsAtAGlance(pr.reviewers);
  // A draft's next step is Mark ready, never Merge: the button below picks one.
  const canMerge = strip.mergeBlocked === null;
  const blocked = strip.mergeBlocked ?? [];
  const mayApprove = strip.canApprove || strip.approved;
  const draft = pr.draft === true;
  const ready = readiness(draft, strip.mergeBlocked);
  const ReadyIcon = READY_ICON[ready.kind];
  const Icon = draft ? GitPullRequestDraft : GitPullRequest;
  const menu: ChatMenuItem[] = [
    { id: "open", label: "Open in Bitbucket", icon: <ExternalLink size={14} />, onSelect: () => void openExternal(pr.url) },
    ...(mayApprove
      ? [
          {
            id: "approve",
            label: strip.approved ? "Unapprove" : "Approve",
            icon: <Check size={14} />,
            disabled: busy,
            onSelect: () => void run(() => approve(!strip.approved)),
          },
        ]
      : []),
    // A server before drafts says neither, and has nothing to convert to.
    ...(pr.draft !== false
      ? []
      : [
          {
            id: "draft",
            label: "Convert to draft",
            hint: "Reviewers still see it; nothing merges it until it is marked ready",
            icon: <Undo2 size={14} />,
            divided: true,
            disabled: busy,
            onSelect: () => void run(() => setDraft(pr.id, pr.version, true), "Converted to draft"),
          },
        ]),
  ];
  return (
    <div className={error ? "pr-stale" : undefined}>
      {build}
      <div className="pr-card">
        <button type="button" className="pr-row pr-head" aria-expanded={open} title={pr.title} onClick={() => setOpen(!open)}>
          <Icon size={14} aria-label={draft ? "draft pull request" : "pull request"} />
          <span className="pr-lines">
            <span className="pr-name">{pr.title}</span>
            <span className="pr-sub">
              {/* A long repository name gives way first; the number stays. */}
              <span className="pr-where" title={`${strip.repo} #${pr.id}`}>
                <span className="pr-repo">{strip.repo}</span>
                <span>&nbsp;#{pr.id}</span>
              </span>
              {draft && <span className="pr-draft">Draft</span>}
              {reviews && <span aria-hidden>·</span>}
              {reviews && (
                <span className="pr-glance" title={`${pr.reviewers.filter((r) => r.status === "approved").length} of ${pr.reviewers.length} reviewers approved`}>
                  {reviews}
                </span>
              )}
            </span>
            {/* A line of its own, its icon in the column of the build's above. */}
            <span className="pr-status" title={ready.text}>
              <span className="pr-status-text">{ready.label}</span>
              <span className="pr-ready-box">
                <ReadyIcon size={12} className={`pr-ready ${ready.kind}`} role="img" aria-label={ready.text} />
              </span>
            </span>
          </span>
        </button>
        <div className="pr-menu">
          <ChatMenu items={menu} icon={<MoreHorizontal size={15} />} />
        </div>
      </div>
      {draft ? (
        <button type="button" className="pr-next" disabled={busy} onClick={() => void run(() => setDraft(pr.id, pr.version, false), "Marked ready for review")}>
          Mark ready
        </button>
      ) : (
        canMerge && (
          <button type="button" className="pr-next" disabled={busy} onClick={() => setConfirm("merge")}>
            <GitMerge size={12} /> Merge
          </button>
        )
      )}
      {failed}

      {open && (
        <div className="pr-more">
          <div className="pr-meta">
            <span title={pr.author}>{pr.authorName}</span> → {pr.toBranch}
          </div>
          {pr.reviewers.length > 0 && <Reviewers reviewers={pr.reviewers} />}
          {blocked.length > 0 && (
            <ul className="pr-vetoes" aria-label="Why it cannot be merged">
              {blocked.map((veto) => (
                <li key={veto}>{veto}</li>
              ))}
            </ul>
          )}
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

/** Past this many, the rest wait behind "N more". */
const REVIEWERS_SHOWN = 5;

/** One reviewer a line, who holds it up first, the status in words at the end; the login under the pointer. */
function Reviewers({ reviewers }: { reviewers: PullRequest["reviewers"] }) {
  const [all, setAll] = useState(false);
  const ordered = reviewersInOrder(reviewers);
  // Folding one name away saves no line: "1 more" takes its place.
  const shown = all || ordered.length <= REVIEWERS_SHOWN + 1 ? ordered : ordered.slice(0, REVIEWERS_SHOWN);
  return (
    <ul className="pr-reviewers" aria-label="Reviewers">
      {shown.map((r) => {
        const Mark = REVIEW_ICON[r.status];
        return (
          <li key={r.slug} className={r.status} title={r.slug}>
            <Mark size={12} />
            <span className="pr-reviewer-name">{r.displayName}</span>
            <span className="pr-reviewer-status">{REVIEW_WORD[r.status]}</span>
          </li>
        );
      })}
      {shown.length < ordered.length && (
        <li>
          <button type="button" className="pr-reviewers-more" onClick={() => setAll(true)}>
            {ordered.length - shown.length} more
          </button>
        </li>
      )}
    </ul>
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
  onCreate: (title: string, to: string | null, draft: boolean) => void;
}) {
  const [title, setTitle] = useState("");
  const [to, setTo] = useState(target);
  const [draft, setDraft] = useState(false);
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
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => onCreate(title, to.trim() || null, draft)}>
            {draft ? "Create draft" : "Create"}
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
      <label className="pr-create-draft">
        <span>As a draft — reviewers see it, nothing merges it until it is marked ready</span>
        <Switch label="Create as draft" on={draft} onChange={setDraft} />
      </label>
      <p className="modal-note">
        From {branch}. The pull request is opened on the server, where it stays until it is declined there.
      </p>
    </Modal>
  );
}
