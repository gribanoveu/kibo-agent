import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  Circle,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleHelp,
  CircleX,
  ExternalLink,
  GitCompare,
  GitMerge,
  GitPullRequest,
  GitPullRequestCreate,
  GitPullRequestDraft,
  Hammer,
  Loader2,
  MessagesSquare,
  MoreHorizontal,
  RefreshCw,
  TriangleAlert,
  Undo2,
} from "lucide-react";
import { Modal } from "./Modal";
import { CodeHostConnect } from "./CodeHostConnect";
import { ChatMenu, type ChatMenuItem } from "./ChatMenu";
import { Dropdown } from "./Dropdown";
import { Switch } from "./Switch";
import { matches } from "../lib/shortcuts";
import type { BranchRef } from "../lib/git";
import { openExternal } from "../lib/window";
import { addressCommentsRequest, buildText, offersConnect, prTasksOf, readiness, resolveConflictsRequest, REVIEW_WORD, reviewersInOrder, reviewsAtAGlance } from "../lib/pullRequestView";
import type { Build, BuildState, PrStrip, PullRequest, ReviewStatus } from "../lib/pullRequests";
import type { PullRequestState } from "../hooks/usePullRequest";
import "./PullRequestStrip.css";

type Props = PullRequestState & { onNotify: (msg: string) => void; onTask?: (text: string) => void };
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
 * (docs/62-bitbucket-prs.md, decision 5): its title, "repo #id" and the
 * reviews under it, a dot for whether it can go in, the "⋯" of what else can
 * be done, and a row of buttons under it — the next step (Mark ready on a draft,
 * Merge while the server allows it) and Open, that pull request's page in the
 * browser, its label kept to one word and the server and number in its
 * tooltip: the two fit a narrow panel this way. Clicked, it opens into its
 * reviewers and why it
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
          onFetch={props.fetchCert}
        />
        {failed}
      </>
    );
  }

  return <ReadyRow {...props} strip={strip} failed={failed} />;
}

function ReadyRow({ strip, error, create, approve, merge, setDraft, onNotify, onTask, branches, loadBranches, failed }: Props & { strip: Ready; failed: ReactNode }) {
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
          branches={branches}
          onLoadBranches={loadBranches}
          busy={busy}
          onClose={() => setConfirm(null)}
          onCreate={(title, to, draft) => void run(() => create(title, to, draft), draft ? "Draft pull request created" : "Pull request created")}
        />
      </div>
    );
  }

  const reviews = reviewsAtAGlance(pr.reviewers);
  // The work the pull request asks of the agent: conflicts and open remarks.
  const tasks = prTasksOf(strip);
  // A draft's next step is Mark ready, never Merge: the button below picks one.
  const canMerge = strip.mergeBlocked === null;
  const blocked = strip.mergeBlocked ?? [];
  const mayApprove = strip.canApprove || strip.approved;
  const draft = pr.draft === true;
  const ready = readiness(draft, strip.mergeBlocked);
  const ReadyIcon = READY_ICON[ready.kind];
  const Icon = draft ? GitPullRequestDraft : GitPullRequest;
  const menu: ChatMenuItem[] = [
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
        <div className="pr-row pr-head">
          <Icon size={14} aria-label={draft ? "draft pull request" : "pull request"} />
          <span className="pr-lines">
            {/* The head is not one button: "N/M approved" on the line under the
                title is a button of its own, and a button may not hold one. */}
            <button type="button" className="pr-name pr-head-toggle" aria-expanded={open} title={pr.title} onClick={() => setOpen(!open)}>
              {pr.title}
            </button>
            <span className="pr-sub" title={`${strip.repo} #${pr.id}`}>
              {/* Neither the number nor the repository is in the line — both only
                  cut it short — they wait under the pointer, as the folder is open. */}
              {draft && <span className="pr-draft">Draft</span>}
              {reviews && (
                <>
                  {/* A separator only where something stands before it. */}
                  {draft && <span aria-hidden>·</span>}
                  <ReviewsPopover reviewers={pr.reviewers} glance={reviews} />
                </>
              )}
              {(draft || reviews) && <span aria-hidden>·</span>}
              {/* Whether it can go in, at the end of the same line: the head is
                  the title and one line under it. */}
              <span className="pr-status" title={ready.text}>
                <span className="pr-status-text">{ready.label}</span>
                <span className="pr-ready-box">
                  <ReadyIcon size={12} className={`pr-ready ${ready.kind}`} role="img" aria-label={ready.text} />
                </span>
              </span>
            </span>
          </span>
        </div>
        {/* Nothing left to do in here — a draft on a server before approvals —
            leaves no "⋯" at all. */}
        {menu.length > 0 && (
          <div className="pr-menu">
            <ChatMenu items={menu} icon={<MoreHorizontal size={15} />} />
          </div>
        )}
      </div>
      {/* The next step and the way to the pull request's page, side by side: a
          link to the page is not something the "⋯" should hide. */}
      <div className="pr-actions">
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
        <button
          type="button"
          className="pr-next"
          title={`Open ${strip.repo} #${pr.id} in Bitbucket`}
          onClick={() => void openExternal(pr.url)}
        >
          <ExternalLink size={12} /> Open
        </button>
      </div>
      {/* The agent's tasks, each putting its request in the message box — the
          user reads it and sends, as a review's Fix does. */}
      {onTask && tasks && (
        <div className="pr-tasks">
          {tasks.conflicted && (
            <button type="button" className="pr-next" onClick={() => onTask(resolveConflictsRequest(pr))}>
              <GitCompare size={12} /> Resolve conflicts
            </button>
          )}
          {tasks.comments.length > 0 && (
            <button type="button" className="pr-next" onClick={() => onTask(addressCommentsRequest(pr, tasks.comments))}>
              <MessagesSquare size={12} /> Address comments
            </button>
          )}
        </div>
      )}
      {failed}

      {open && (
        <div className="pr-more">
          <div className="pr-meta">
            <span title={pr.author}>{pr.authorName}</span> → {pr.toBranch}
          </div>
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

/** The reviewers' names past this many wait behind "N more". */
const REVIEWERS_SHOWN = 5;

/**
 * "1/2 approved" — a button, and the reviewers themselves in a popover pinned
 * to it: the count does not say who holds the pull request up, and the list is
 * wanted without opening the whole row for it.
 *
 * Portaled to the viewport like the "⋯" menu, and for the same reason: the
 * card sits in a panel that scrolls, which would clip it and carry it away.
 */
function ReviewsPopover({ reviewers, glance }: { reviewers: PullRequest["reviewers"]; glance: string }) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const popover = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const close = useCallback(() => {
    setOpen(false);
    setPos(null);
  }, []);

  // Below the line, flipped above when the window's lower edge is nearer,
  // never past its right.
  useLayoutEffect(() => {
    if (!open) return;
    const button = trigger.current?.getBoundingClientRect();
    const box = popover.current?.getBoundingClientRect();
    if (!button || !box) return;
    const below = button.bottom + 6;
    const top = below + box.height > window.innerHeight - 8 && button.top - 6 - box.height >= 8 ? button.top - 6 - box.height : below;
    const left = Math.max(8, Math.min(button.left, window.innerWidth - box.width - 8));
    setPos((was) => (was && Math.abs(was.top - top) < 0.5 && Math.abs(was.left - left) < 0.5 ? was : { top, left }));
  });

  // Dismissed as every other popup here is — an outside press or Escape. A
  // scroll of what holds the trigger closes it: the trigger has moved.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!trigger.current?.contains(e.target as Node) && !popover.current?.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (!matches(e, "close")) return;
      e.preventDefault();
      close();
    };
    const onScroll = (e: Event) => {
      if (e.target instanceof Node && trigger.current && e.target.contains(trigger.current)) close();
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
    };
  }, [open, close]);

  const approved = reviewers.filter((reviewer) => reviewer.status === "approved").length;
  return (
    <>
      <button
        type="button"
        ref={trigger}
        className="pr-glance"
        title={`${approved} of ${reviewers.length} reviewers approved`}
        aria-label={`Reviewers — ${approved} of ${reviewers.length} approved`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => {
          if (open) {
            close();
            return;
          }
          // Somewhere on screen at once: the popover places itself once drawn.
          const button = trigger.current?.getBoundingClientRect();
          setPos(button ? { top: button.bottom + 6, left: button.left } : null);
          setOpen(true);
        }}
      >
        {glance}
      </button>
      {open &&
        pos &&
        createPortal(
          <div ref={popover} className="pr-reviewers-pop" role="dialog" aria-label="Reviewers" style={{ position: "fixed", top: pos.top, left: pos.left }}>
            <Reviewers reviewers={reviewers} />
          </div>,
          document.body,
        )}
    </>
  );
}

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

/**
 * The branches a pull request can go into: this folder's own, the default one
 * first — it is where a pull request goes — and never the branch it comes from,
 * a pull request into itself being nothing. The default stays listed even where
 * only its remote side is known: the server has that branch either way.
 */
function intoBranches(branches: BranchRef[], target: string, from: string): { value: string; hint?: string }[] {
  const local = branches.filter((branch) => !branch.remote).map((branch) => branch.name);
  const rest = local.filter((name) => name !== target && name !== from).map((value) => ({ value }));
  return target && target !== from ? [{ value: target, hint: "default" }, ...rest] : rest;
}

function CreateDialog({
  open,
  branch,
  target,
  branches,
  onLoadBranches,
  busy,
  onClose,
  onCreate,
}: {
  open: boolean;
  branch: string;
  target: string;
  branches: BranchRef[];
  onLoadBranches: () => void;
  busy: boolean;
  onClose: () => void;
  onCreate: (title: string, to: string | null, draft: boolean) => void;
}) {
  const [title, setTitle] = useState("");
  const [to, setTo] = useState(target);
  const [draft, setDraft] = useState(false);
  // Read when the dialog opens: the branches asked for are the folder's at this
  // moment, not whatever it had when the strip was drawn.
  useEffect(() => {
    if (open) onLoadBranches();
  }, [open, onLoadBranches]);
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
        <label>Into</label>
        {/* The branches this folder has, the default one first — and a branch not
            read here can still be typed, the server being the one that knows. */}
        <div className="pr-create-into">
          <Dropdown
            label={to}
            title="The branch the pull request goes into"
            options={intoBranches(branches, target, branch)}
            value={to}
            onPick={setTo}
            below
            custom={{ placeholder: "Another branch", onEnter: setTo, onType: setTo }}
            emptyLabel="No branch read yet"
          />
        </div>
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
