import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";

// The pull request of the open folder's branch, and the servers it is read
// from (docs/62-bitbucket-prs.md). Mirrors `domain::code_host` and
// `commands/pull_requests.rs`.

/** A server the user added. Never its token: only whether one is set. */
export type CodeHostServer = {
  id: string;
  baseUrl: string;
  /** Hosts it also answers to, learnt from a clone link — an ssh host apart from the web one. */
  hosts: string[];
  trustedCertPem: string | null;
  tokenSet: boolean;
};

/** A remote read into what a server needs; `baseUrl` is the form's guess at the server. */
export type RemoteRepo = { host: string; baseUrl: string; project: string; slug: string; transport: "web" | "ssh" };

export type ReviewStatus = "approved" | "unapproved" | "needsWork";
/** `slug` is the login the server matches on; `displayName` is what is shown — the slug when the server gives none. */
export type Reviewer = { slug: string; displayName: string; status: ReviewStatus };

export type PullRequest = {
  id: number;
  title: string;
  version: number;
  /** The login the server matches on. */
  author: string;
  /** What is shown — the login when the server gives no name. */
  authorName: string;
  fromBranch: string;
  toBranch: string;
  reviewers: Reviewer[];
  /** The page Open opens. */
  url: string;
  /** Work in progress: not merged until marked ready. `null` on a server before 8.18, which has no drafts. */
  draft: boolean | null;
};

/** `unknown` is a state this app has no word for — never a success. */
export type BuildState = "successful" | "failed" | "inProgress" | "unknown";
/** `dateAdded`: when the server took this status, epoch ms; `null` when it does not say. */
export type Build = { key: string; name: string; state: BuildState; url: string; dateAdded: number | null };

export type MergeCheck = { canMerge: boolean; conflicted: boolean; vetoes: string[] };

/** A reviewer's remark (`domain::code_host::Comment`): a comment thread or a task, with its place when inline. */
export type PrComment = {
  id: number;
  /** The login the server matches on. */
  author: string;
  /** What is shown — the login when the server gives no name. */
  authorName: string;
  text: string;
  path: string | null;
  line: number | null;
  /** A task (blocker comment) rather than an ordinary comment. */
  task: boolean;
  /** The thread is resolved, or the task RESOLVED: no longer open. */
  resolved: boolean;
};

export type Snapshot = {
  pr: PullRequest | null;
  branchOnServer: boolean;
  defaultBranch: string | null;
  merge: MergeCheck | null;
  builds: Build[];
  /** The reviewers' remarks, open and resolved. */
  comments: PrComment[];
};

/** What the strip shows for the open folder (`domain::code_host::PrStrip`). */
export type PrStrip =
  | { state: "noRemote" }
  | { state: "noBranch" }
  | { state: "unreachable" }
  | { state: "notConnected"; remote: RemoteRepo }
  | {
      state: "ready";
      serverId: string;
      /** The repository's slug, for the line under the title. */
      repo: string;
      branch: string;
      snapshot: Snapshot;
      canApprove: boolean;
      approved: boolean;
      /** Why Merge is off, in the server's words; `null` with a pull request is a live Merge. */
      mergeBlocked: string[] | null;
      /** The newest build of the branch's commit — the one shown. */
      latestBuild: Build | null;
      /** The open remarks: Address comments shows only while it is more than none. */
      openComments: number;
    };

/**
 * The strip of the folder `root` changed, on the server or by a button: read
 * it again. Pinned on the Rust side in `commands/pull_requests.rs`.
 */
export const PULL_REQUESTS_EVENT = "pull-requests:changed";

export async function onPullRequestChanged(handler: (root: string) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ root: string }>(PULL_REQUESTS_EVENT, ({ payload }) => handler(payload.root));
}

export async function codeHosts(): Promise<CodeHostServer[]> {
  if (!inTauri()) return [];
  return invoke<CodeHostServer[]>("code_hosts");
}

/**
 * Saves a server; `token` of `null` keeps the saved one. `forFolder`: the
 * strip's connect form — the open folder's repository is proven to be on it.
 */
export async function codeHostSave(server: CodeHostServer, token: string | null, forFolder: boolean): Promise<CodeHostServer[]> {
  requireBackend();
  return invoke<CodeHostServer[]>("code_host_save", { server, token, forFolder });
}

export async function codeHostDelete(id: string): Promise<CodeHostServer[]> {
  requireBackend();
  return invoke<CodeHostServer[]>("code_host_delete", { id });
}

/** What the address answers as, asked without a token: "Bitbucket 8.19.26". */
export async function codeHostDetect(server: CodeHostServer): Promise<string> {
  requireBackend();
  return invoke<string>("code_host_detect", { server });
}

/** What a server presented on a handshake that asked nothing else (BB-10). */
export type FetchedCert = {
  /** The most root-ward certificate it sent, PEM. */
  pem: string;
  /** SHA-256 of its DER, as openssl prints it: uppercase hex pairs, colon between. */
  fingerprint: string;
  /** How many certificates it sent. */
  chain: number;
};

/**
 * The certificate the address presents, without a token and without trusting
 * it: the form shows the fingerprint and asks before it is kept.
 */
export async function codeHostFetchCert(baseUrl: string): Promise<FetchedCert> {
  requireBackend();
  return invoke<FetchedCert>("code_host_fetch_cert", { baseUrl });
}

export async function pullRequest(): Promise<PrStrip> {
  if (!inTauri()) return { state: "noRemote" };
  return invoke<PrStrip>("pull_request");
}

/** From the checked-out branch into `to`, or the repository's default branch. */
export async function pullRequestCreate(title: string, to: string | null, draft: boolean): Promise<PullRequest> {
  requireBackend();
  return invoke<PullRequest>("pull_request_create", { title, to, draft });
}

/** What the last read of the open folder gave, asking no server — `null` before one, or after one that failed. */
export async function pullRequestLast(): Promise<PrStrip | null> {
  requireBackend();
  return invoke<PrStrip | null>("pull_request_last");
}

/** `false` marks the draft ready for review; `true` takes it back to a draft. */
export async function pullRequestSetDraft(id: number, version: number, draft: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("pull_request_set_draft", { id, version, draft });
}

/** `false` takes the approval back. */
export async function pullRequestApprove(approved: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("pull_request_approve", { approved });
}

/** The pull request the user confirmed, at the `version` the strip showed: one that moved since is refused. */
export async function pullRequestMerge(id: number, version: number): Promise<void> {
  requireBackend();
  return invoke<void>("pull_request_merge", { id, version });
}

/** The strip is on screen for the open folder. */
export async function pullRequestWatch(): Promise<void> {
  if (!inTauri()) return;
  return invoke<void>("pull_request_watch");
}

/** The strip of `root` is gone — the folder it watched, which may no longer be the open one. */
export async function pullRequestUnwatch(root: string): Promise<void> {
  if (!inTauri()) return;
  return invoke<void>("pull_request_unwatch", { root });
}

/** `.git` changed: whether the strip should read again — only a switch of branch, a push or a fetch is news to the server. */
export async function pullRequestGitChanged(): Promise<boolean> {
  if (!inTauri()) return false;
  return invoke<boolean>("pull_request_git_changed");
}
