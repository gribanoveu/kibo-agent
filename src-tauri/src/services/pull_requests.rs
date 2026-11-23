//! The pull request of the open folder's branch: the strip's snapshot, its
//! three buttons, the connect form's checks, and the watcher that keeps the
//! strip current (`docs/62-bitbucket-prs.md`, BB-3).
//!
//! The server is the one the user added (`code_host_store`), found by the
//! folder's remote (`domain::code_host::server_for`); the host is reached only
//! through [`CodeHost`]. What changes goes out through a
//! [`PullRequestEventSink`] — a signal to read again, never the snapshot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};

use crate::domain::code_host::{
    alias_to_remember, approved_by, can_approve, canonical_remote, host_of, latest_build, merge_blocked, open_comments, server_for, Call,
    CodeHost, CodeHostError, CodeHostServer, PrStrip, PullRequest, PullRequestChanged, PullRequestEventSink, AgentPullRequests, RemoteRepo,
    Snapshot,
};
use crate::infra::{code_host_store, git_head, repository_identity};

/// How often the watcher asks while nothing is running — a read is seven
/// requests of under a second each on the probed server (BB-0, step 4).
const QUIET: Duration = Duration::from_secs(30);
/// How often while a build runs: that is when the strip is being watched.
const BUILDING: Duration = Duration::from_secs(10);

pub struct PullRequests {
    host: Arc<dyn CodeHost>,
    changed: PullRequestEventSink,
    /// The token's user, per server, with the fingerprint of the token that
    /// was asked: one request per token rather than one per read, and a read
    /// begun with the old token cannot leave its user for the new one.
    me: Mutex<HashMap<String, (blake3::Hash, String)>>,
    /// The window's last read, watched or not: a watch that starts after it
    /// starts from what the window already has.
    read_last: Mutex<Option<(PathBuf, Read)>>,
    /// What the server could know of the repository when the window last
    /// read: a change to `.git` is a reason to ask it only when this moved.
    pushed: Mutex<Option<(PathBuf, Local)>>,
    watching: Mutex<Option<Watching>>,
    quiet: Duration,
    building: Duration,
}

/// The one folder being watched: the strip is on screen for it.
struct Watching {
    root: PathBuf,
    /// Sent on when the window reads, so the thread counts its wait from
    /// there and with what that read says about builds; dropped, it stops the
    /// thread at once rather than at its next tick.
    wake: Sender<()>,
    /// What the window was last given — by its own read or the watcher's —
    /// so that the event says "differs from what you have".
    last: Arc<Mutex<Option<Read>>>,
}

type Read = Result<PrStrip, CodeHostError>;

/// The remote as written, the branch and where each remote's copy of it
/// points: what a server could have heard of.
type Local = (Option<String>, Option<(String, Vec<String>)>);

fn local(root: &Path) -> Local {
    (repository_identity::remote_url(root), git_head::pushed_state(root))
}

/// Where a folder's requests go, once a server is found for it.
struct Target {
    server: CodeHostServer,
    token: SecretString,
    remote: RemoteRepo,
    branch: String,
}

impl Target {
    fn call(&self) -> Call<'_> {
        Call { server: &self.server, token: self.token.expose_secret(), remote: &self.remote }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PullRequests {
    pub fn new(host: Arc<dyn CodeHost>, changed: PullRequestEventSink) -> Self {
        Self::with_ticks(host, changed, QUIET, BUILDING)
    }

    fn with_ticks(host: Arc<dyn CodeHost>, changed: PullRequestEventSink, quiet: Duration, building: Duration) -> Self {
        Self {
            host,
            changed,
            me: Mutex::default(),
            read_last: Mutex::default(),
            pushed: Mutex::default(),
            watching: Mutex::default(),
            quiet,
            building,
        }
    }

    // ---------------------------------------------------------------- reading

    /// What the strip shows for `root`. A server that answers wrongly is an
    /// `Err`: the strip keeps its last snapshot grey with the reason. One that
    /// cannot be reached — off the VPN, say — is [`PrStrip::Unreachable`], and
    /// the strip shows nothing until it answers again.
    pub fn strip(&self, root: &Path) -> Read {
        *lock(&self.pushed) = Some((root.to_path_buf(), local(root)));
        let read = self.read(root);
        *lock(&self.read_last) = Some((root.to_path_buf(), read.clone()));
        if let Some(watching) = lock(&self.watching).as_ref().filter(|watching| watching.root == root) {
            *lock(&watching.last) = Some(read.clone());
            // A thread that has stopped is no error: the watch is being replaced.
            let _ = watching.wake.send(());
        }
        read
    }

    /// What the last read of `root` gave, asking nothing: the strip opens with
    /// it while the server is asked again. A read that failed is not kept for
    /// this — the strip has nothing grey to keep yet.
    pub fn last(&self, root: &Path) -> Option<PrStrip> {
        match lock(&self.read_last).as_ref() {
            Some((read_root, Ok(strip))) if read_root == root => Some(strip.clone()),
            _ => None,
        }
    }

    /// `.git` changed: whether the strip should read the server again — the
    /// remote was added or changed, the branch switched, pushed or fetched
    /// since its last read. Staging, a local commit or an editor's own
    /// `git status` cost the server nothing.
    pub fn git_changed(&self, root: &Path) -> bool {
        let now = local(root);
        let seen = lock(&self.pushed);
        seen.as_ref().is_none_or(|(seen_root, seen)| seen_root != root || *seen != now)
    }

    fn read(&self, root: &Path) -> Read {
        match self.ask(root, None) {
            Err(CodeHostError::Unreachable(_)) => Ok(PrStrip::Unreachable),
            read => read,
        }
    }

    fn ask(&self, root: &Path, branch: Option<&str>) -> Read {
        let target = match target(root, branch)? {
            Found::Target(target) => target,
            Found::Nothing(strip) => return Ok(strip),
        };
        let call = target.call();
        let host = self.host.as_ref();
        // Three rounds rather than seven requests in a row: each round waits
        // on the one before it and on nothing else. A read costs the slowest of
        // each round — the merge check, as a rule — not the sum.
        let (default_branch, commit) = both(|| host.default_branch(&call), || host.branch_commit(&call, &target.branch));
        let (default_branch, commit) = (default_branch?, commit?);
        // A branch the server does not have has no pull request either: the
        // strip says "push first" and asks nothing more.
        let (pr, builds) = match &commit {
            Some(commit) => {
                let (pr, builds) = both(|| host.pull_request(&call, &target.branch), || host.builds(&call, commit));
                (pr?, builds?)
            }
            None => (None, Vec::new()),
        };
        let (merge, comments, me) = match &pr {
            Some(pr) => {
                let (merge, comments, me) = three(|| host.merge_check(&call, pr.id), || host.comments(&call, pr.id), || self.me(&call));
                (Some(merge?), comments?, me?)
            }
            None => (None, Vec::new(), None),
        };
        let (can_approve, approved) = match (&pr, &me) {
            (Some(pr), Some(me)) => (can_approve(pr, me), approved_by(pr, me)),
            _ => (false, false),
        };
        Ok(PrStrip::Ready {
            server_id: target.server.id.clone(),
            repo: target.remote.slug.clone(),
            branch: target.branch.clone(),
            merge_blocked: merge.as_ref().and_then(merge_blocked),
            latest_build: latest_build(&builds).cloned(),
            open_comments: open_comments(&comments),
            snapshot: Snapshot { pr, branch_on_server: commit.is_some(), default_branch: Some(default_branch), merge, builds, comments },
            can_approve,
            approved,
        })
    }

    /// The token's user, asked once per server. A server that does not say is
    /// `None` — no Approve, rather than no strip.
    fn me(&self, call: &Call) -> Result<Option<String>, CodeHostError> {
        let token = blake3::hash(call.token.as_bytes());
        if let Some((_, me)) = lock(&self.me).get(&call.server.id).filter(|(asked, _)| *asked == token) {
            return Ok(Some(me.clone()));
        }
        match self.host.me(call) {
            Ok(me) => {
                lock(&self.me).insert(call.server.id.clone(), (token, me.clone()));
                Ok(Some(me))
            }
            Err(CodeHostError::BadAnswer(_)) => Ok(None),
            Err(other) => Err(other),
        }
    }

    // ---------------------------------------------------------------- buttons

    /// Approve, or take an approval back.
    pub fn approve(&self, root: &Path, approved: bool) -> Result<(), CodeHostError> {
        self.acting(root, |call, target| {
            let pr = self.open_pull_request(call, target)?;
            let me = self
                .me(call)?
                .ok_or_else(|| CodeHostError::BadAnswer("the server did not say whose token this is".to_string()))?;
            self.host.approve(call, &pr, &me, approved)
        })
    }

    /// Merges the pull request the user confirmed — its `id` at the `version`
    /// the strip showed. One that has moved since is [`CodeHostError::Conflict`]
    /// before anything is sent: a merge cannot be taken back, and what is
    /// merged is what was seen. The server's own 409 covers the moment between
    /// this read and the merge; the strip reads again either way.
    pub fn merge(&self, root: &Path, id: u64, version: u64) -> Result<(), CodeHostError> {
        self.acting(root, |call, target| {
            let pr = self.open_pull_request(call, target)?;
            if (pr.id, pr.version) != (id, version) {
                return Err(CodeHostError::Conflict);
            }
            self.host.merge(call, &pr)
        })
    }

    /// Marks the draft the user saw ready for review (`draft: false`), or takes
    /// it back to a draft — at the `version` the strip showed, as merge is.
    pub fn set_draft(&self, root: &Path, id: u64, version: u64, draft: bool) -> Result<(), CodeHostError> {
        self.acting(root, |call, target| {
            let pr = self.open_pull_request(call, target)?;
            if (pr.id, pr.version) != (id, version) {
                return Err(CodeHostError::Conflict);
            }
            self.host.set_draft(call, &pr, draft)
        })
    }

    /// Opens a pull request from the checked-out branch into `to`, or into the
    /// repository's default branch. An empty title is the branch's name.
    /// `refs/heads/main` is `main`: the server prefixes the name itself.
    pub fn create(&self, root: &Path, title: &str, description: &str, to: Option<&str>, draft: bool) -> Result<PullRequest, CodeHostError> {
        self.acting(root, |call, target| {
            if self.host.branch_commit(call, &target.branch)?.is_none() {
                return Err(CodeHostError::NotPushed(target.branch.clone()));
            }
            let to = to.map(|to| to.trim()).map(|to| to.strip_prefix("refs/heads/").unwrap_or(to));
            let to = match to.filter(|to| !to.is_empty()) {
                Some(to) => to.to_string(),
                None => self.host.default_branch(call)?,
            };
            let title = match title.trim() {
                "" => target.branch.as_str(),
                title => title,
            };
            self.host.create(call, &target.branch, &to, title, description, draft)
        })
    }

    fn open_pull_request(&self, call: &Call, target: &Target) -> Result<PullRequest, CodeHostError> {
        self.host.pull_request(call, &target.branch)?.ok_or_else(|| CodeHostError::NoPullRequest(target.branch.clone()))
    }

    /// An action, and then the signal to read again whatever came of it: a
    /// refusal is as much news as a success — the version moved, a veto came.
    fn acting<T>(&self, root: &Path, act: impl FnOnce(&Call, &Target) -> Result<T, CodeHostError>) -> Result<T, CodeHostError> {
        let target = match target(root, None)? {
            Found::Target(target) => target,
            Found::Nothing(PrStrip::NoBranch) => return Err(CodeHostError::NoBranch),
            Found::Nothing(PrStrip::NotConnected { remote }) => return Err(CodeHostError::NoServer(remote.host)),
            Found::Nothing(_) => return Err(CodeHostError::NoRemote),
        };
        let result = act(&target.call(), &target);
        self.report(root);
        result
    }

    // --------------------------------------------------------------- servers

    /// What the server at this address says it is, without a token.
    pub fn identify(&self, server: &CodeHostServer) -> Result<String, CodeHostError> {
        self.host.identify(server)
    }

    /// Saves a server; with `root`, the folder the connect form was opened for,
    /// first proves the folder's repository is on it — one of its clone links
    /// is the remote — and learns the remote's host as the server's alias
    /// when it is not the server's own (decision 1).
    ///
    /// Nothing is saved when the proof fails: a form sent again would
    /// otherwise add a second server on the same host, and two servers on one
    /// host find neither. Aliases are learnt, never typed: an edit keeps the
    /// ones the saved server has, whatever the form sent.
    pub fn save(&self, server: CodeHostServer, token: Option<String>, root: Option<&Path>) -> Result<Vec<CodeHostServer>, CodeHostError> {
        let mut server = checked(server)?;
        let saved = code_host_store::list()?.into_iter().find(|saved| saved.id == server.id);
        let known = saved.is_some();
        server.hosts = saved.map(|saved| saved.hosts).unwrap_or_default();
        if let Some(root) = root {
            let proof_token = match token.as_deref().map(str::trim).filter(|token| !token.is_empty()) {
                Some(token) => SecretString::from(token),
                None if known => code_host_store::token(&server.id)?,
                None => return Err(CodeHostError::NoToken(server.base_url)),
            };
            if let Some(alias) = self.prove(&server, &proof_token, root)? {
                server.hosts.push(alias);
            }
        }
        code_host_store::put(server, token)?;
        self.report_changed(root);
        code_host_store::list()
    }

    /// The alias to learn once the clone links have shown the repository on
    /// `server`; `Ok(None)` for a folder whose remote names no repository.
    fn prove(&self, server: &CodeHostServer, token: &SecretString, root: &Path) -> Result<Option<String>, CodeHostError> {
        let Some(remote) = repository_identity::remote_url(root).and_then(|url| RemoteRepo::parse(&url).ok()) else {
            return Ok(None);
        };
        let call = Call { server, token: token.expose_secret(), remote: &remote };
        let wanted = remote.canonical();
        let listed = self.host.clone_links(&call)?;
        if !listed.iter().any(|link| canonical_remote(link).is_ok_and(|link| link == wanted)) {
            return Err(CodeHostError::NotOnServer(wanted));
        }
        Ok(alias_to_remember(server, &remote))
    }

    pub fn delete(&self, id: &str) -> Result<Vec<CodeHostServer>, CodeHostError> {
        code_host_store::delete(id)?;
        self.report_changed(None);
        code_host_store::list()
    }

    /// A server added, changed or gone changes the strip of the folder it was
    /// for and of the folder on screen — NotConnected to Ready and back.
    fn report_changed(&self, root: Option<&Path>) {
        let watched = lock(&self.watching).as_ref().map(|watching| watching.root.clone());
        if let Some(root) = root {
            self.report(root);
        }
        if let Some(watched) = watched.filter(|watched| Some(watched.as_path()) != root) {
            self.report(&watched);
        }
    }

    // --------------------------------------------------------------- watching

    /// Keeps the strip of `root` current while it is on screen: asks the
    /// server every [`QUIET`], every [`BUILDING`] while a build runs, and says
    /// so only when what it read differs from what the window has.
    ///
    /// ponytail: polled — the server cannot reach a desktop app, and a webhook
    /// needs an address it can. The ceiling is the load of many open windows
    /// on one server; past it, a relay that takes the webhook and pushes.
    pub fn watch(self: &Arc<Self>, root: PathBuf) {
        let mut watching = lock(&self.watching);
        if watching.as_ref().is_some_and(|watching| watching.root == root) {
            return;
        }
        let (wake, woken) = mpsc::channel();
        let seen = lock(&self.read_last).as_ref().filter(|(read_root, _)| *read_root == root).map(|(_, read)| read.clone());
        let last = Arc::new(Mutex::new(seen));
        *watching = Some(Watching { root: root.clone(), wake, last: Arc::clone(&last) });
        drop(watching);

        let this = Arc::clone(self);
        std::thread::spawn(move || this.watched(&root, &woken, &last));
    }

    /// Stops watching `root`. Another folder's watch is left alone: an unwatch
    /// from the folder just left can arrive after the next one's watch.
    pub fn unwatch(&self, root: &Path) {
        let mut watching = lock(&self.watching);
        if watching.as_ref().is_some_and(|watching| watching.root == root) {
            *watching = None;
        }
    }

    fn watched(&self, root: &Path, woken: &Receiver<()>, last: &Mutex<Option<Read>>) {
        loop {
            let building = matches!(&*lock(last), Some(Ok(strip)) if strip.building());
            let wait = if building { self.building } else { self.quiet };
            match woken.recv_timeout(wait) {
                Err(RecvTimeoutError::Timeout) => {}
                // The window just read: the wait starts over from its answer.
                Ok(()) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let read = self.read(root);
            match woken.try_recv() {
                Err(TryRecvError::Empty) => {}
                // The window read while the server was being asked: its answer
                // is newer than this one, and is what the window has.
                Ok(()) => continue,
                // Unwatched meanwhile: nobody to tell.
                Err(TryRecvError::Disconnected) => return,
            }
            let mut seen = lock(last);
            if seen.as_ref() != Some(&read) {
                *seen = Some(read);
                drop(seen);
                self.report(root);
            }
        }
    }

    fn report(&self, root: &Path) {
        (self.changed)(PullRequestChanged { root: root.display().to_string() });
    }
}

/// `a` on a thread of its own and `b` on this one, both answers once both are in.
fn both<A: Send, B>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B) -> (A, B) {
    std::thread::scope(|scope| {
        let first = scope.spawn(a);
        let second = b();
        // A panic over there is one here: the read has no answer to give.
        (first.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)), second)
    })
}

/// [`both`] for three: the merge check, the remarks and the token's user,
/// none waiting on another.
fn three<A: Send, B: Send, C>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send, c: impl FnOnce() -> C) -> (A, B, C) {
    std::thread::scope(|scope| {
        let first = scope.spawn(a);
        let second = scope.spawn(b);
        let third = c();
        (
            first.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            second.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            third,
        )
    })
}

/// A server as the form sent it, made ready to keep: an id for a new one, the
/// address without the slash a user may type after it, and an address that is
/// one. Whether `http` may carry the token is the client's to refuse, per
/// request.
fn checked(server: CodeHostServer) -> Result<CodeHostServer, CodeHostError> {
    let base_url = server.base_url.trim().trim_end_matches('/').to_string();
    let scheme = base_url.split_once("://").map(|(scheme, _)| scheme.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("https" | "http")) || host_of(&base_url).is_none() {
        return Err(CodeHostError::BadAddress(server.base_url));
    }
    let id = match server.id.trim() {
        "" => uuid::Uuid::new_v4().to_string(),
        id => id.to_string(),
    };
    Ok(CodeHostServer { id, base_url, ..server })
}

/// The agent's read: what the strip would show, for any branch, and none of
/// the strip's own state touched — its last read, its watcher. A server out
/// of reach is said, not hidden: the model asked, and the answer is that.
impl AgentPullRequests for PullRequests {
    fn status(&self, root: &Path, branch: Option<&str>) -> Read {
        self.ask(root, branch)
    }

    fn create(&self, root: &Path, title: &str, description: &str, to: Option<&str>, draft: bool) -> Result<PullRequest, CodeHostError> {
        PullRequests::create(self, root, title, description, to, draft)
    }
}

enum Found {
    Target(Target),
    /// The folder has nothing to ask a server about, and this is what the
    /// strip shows instead.
    Nothing(PrStrip),
}

/// The server, token, repository and branch of `root` — `branch`, or the one
/// checked out — or what the strip shows when there is none. A host no
/// configured server answers to gets no request: the token goes to a server
/// the user added and to no other (decision 1).
fn target(root: &Path, branch: Option<&str>) -> Result<Found, CodeHostError> {
    let Some(remote) = repository_identity::remote_url(root).and_then(|url| RemoteRepo::parse(&url).ok()) else {
        return Ok(Found::Nothing(PrStrip::NoRemote));
    };
    let servers = code_host_store::list()?;
    let Some(server) = server_for(&servers, &remote).cloned() else {
        return Ok(Found::Nothing(PrStrip::NotConnected { remote }));
    };
    let named = branch.map(str::trim).filter(|branch| !branch.is_empty()).map(str::to_string);
    let Some(branch) = named.or_else(|| git_head::checked_out_branch(root)) else {
        return Ok(Found::Nothing(PrStrip::NoBranch));
    };
    let token = code_host_store::token(&server.id)?;
    Ok(Found::Target(Target { server, token, remote, branch }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::code_host::{Build, BuildState, Comment, MergeCheck, ReviewStatus, Reviewer};
    use crate::testing::{temp_dir, with_app_dir};
    use std::time::Instant;

    const REMOTE: &str = "ssh://git@git.server.com/corp/tax-api.git";

    /// A server as a test wants it: every request counted by name, the answers
    /// set beforehand.
    #[derive(Default)]
    struct Double {
        asked: Mutex<Vec<String>>,
        commit: Mutex<Option<String>>,
        pr: Mutex<Option<PullRequest>>,
        builds: Mutex<Vec<Build>>,
        comments: Mutex<Vec<Comment>>,
        links: Mutex<Vec<String>>,
        me: Mutex<Option<String>>,
        merged: Mutex<Option<CodeHostError>>,
        /// How long a call takes to answer, by name: a read in flight, or
        /// a slow server whose rounds are timed.
        slow: Mutex<HashMap<&'static str, Duration>>,
        /// What the default branch answers instead: a server that does not.
        down: Mutex<Option<CodeHostError>>,
        /// The branch each pull request was asked for.
        branches: Mutex<Vec<String>>,
    }

    impl Double {
        fn ask(&self, what: &str) {
            lock(&self.asked).push(what.to_string());
            let slow = lock(&self.slow).get(what).copied();
            std::thread::sleep(slow.unwrap_or_default());
        }
        fn asked(&self, what: &str) -> usize {
            lock(&self.asked).iter().filter(|asked| *asked == what).count()
        }
        fn total(&self) -> usize {
            lock(&self.asked).len()
        }
    }

    impl CodeHost for Double {
        fn identify(&self, _: &CodeHostServer) -> Result<String, CodeHostError> {
            self.ask("identify");
            Ok("Bitbucket 8.19.26".to_string())
        }
        fn clone_links(&self, call: &Call) -> Result<Vec<String>, CodeHostError> {
            self.ask(&format!("clone_links {}", call.token));
            Ok(lock(&self.links).clone())
        }
        fn me(&self, _: &Call) -> Result<String, CodeHostError> {
            self.ask("me");
            lock(&self.me).clone().ok_or_else(|| CodeHostError::BadAnswer("no X-AUSERNAME".to_string()))
        }
        fn default_branch(&self, _: &Call) -> Result<String, CodeHostError> {
            self.ask("default_branch");
            lock(&self.down).clone().map_or(Ok("master".to_string()), Err)
        }
        fn branch_commit(&self, _: &Call, _: &str) -> Result<Option<String>, CodeHostError> {
            self.ask("branch_commit");
            Ok(lock(&self.commit).clone())
        }
        fn pull_request(&self, _: &Call, branch: &str) -> Result<Option<PullRequest>, CodeHostError> {
            self.ask("pull_request");
            lock(&self.branches).push(branch.to_string());
            Ok(lock(&self.pr).clone())
        }
        fn merge_check(&self, _: &Call, _: u64) -> Result<MergeCheck, CodeHostError> {
            self.ask("merge_check");
            Ok(MergeCheck { can_merge: false, conflicted: false, vetoes: vec!["Requires approvals".to_string()] })
        }
        fn builds(&self, _: &Call, _: &str) -> Result<Vec<Build>, CodeHostError> {
            self.ask("builds");
            Ok(lock(&self.builds).clone())
        }
        fn comments(&self, _: &Call, _: u64) -> Result<Vec<Comment>, CodeHostError> {
            self.ask("comments");
            Ok(lock(&self.comments).clone())
        }
        fn approve(&self, _: &Call, _: &PullRequest, me: &str, approved: bool) -> Result<(), CodeHostError> {
            self.ask(&format!("approve {me} {approved}"));
            Ok(())
        }
        fn merge(&self, _: &Call, pr: &PullRequest) -> Result<(), CodeHostError> {
            self.ask(&format!("merge v{}", pr.version));
            lock(&self.merged).clone().map_or(Ok(()), Err)
        }
        fn set_draft(&self, _: &Call, pr: &PullRequest, draft: bool) -> Result<(), CodeHostError> {
            self.ask(&format!("draft {draft} v{}", pr.version));
            Ok(())
        }
        fn create(&self, _: &Call, from: &str, to: &str, title: &str, description: &str, draft: bool) -> Result<PullRequest, CodeHostError> {
            let described = if description.is_empty() { String::new() } else { format!(" [{description}]") };
            self.ask(&format!("create {from} -> {to}: {title}{described}{}", if draft { " (draft)" } else { "" }));
            Ok(pull_request(1))
        }
    }

    fn pull_request(version: u64) -> PullRequest {
        PullRequest {
            id: 153,
            title: "a pull request".to_string(),
            version,
            author: "u_author".to_string(),
            author_name: "Anna Author".to_string(),
            draft: Some(false),
            from_branch: "feature/x".to_string(),
            to_branch: "master".to_string(),
            reviewers: vec![Reviewer { slug: "u_me".to_string(), display_name: "Me".to_string(), status: ReviewStatus::Unapproved }],
            url: "https://server.com/projects/CORP/repos/tax-api/pull-requests/153".to_string(),
        }
    }

    fn build(state: BuildState) -> Build {
        Build { key: "ci".to_string(), name: "build #12".to_string(), state, url: "https://ci.example/12".to_string(), date_added: None }
    }

    fn remark(id: u64, resolved: bool) -> Comment {
        Comment {
            id,
            author: "u_reviewer".to_string(),
            author_name: "Rita Reviewer".to_string(),
            text: "Move this into the builder".to_string(),
            path: Some("src/main.rs".to_string()),
            line: Some(34),
            task: false,
            resolved,
        }
    }

    /// A repository with `remote` as `origin` and `feature/x` checked out.
    fn repo(label: &str, remote: Option<&str>) -> PathBuf {
        let dir = temp_dir(label);
        let repo = git2::Repository::init(&dir).unwrap();
        if let Some(remote) = remote {
            repo.remote("origin", remote).unwrap();
        }
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        let commit = repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[]).unwrap();
        repo.branch("feature/x", &repo.find_commit(commit).unwrap(), false).unwrap();
        repo.set_head("refs/heads/feature/x").unwrap();
        dir
    }

    fn server(base_url: &str, hosts: &[&str]) -> CodeHostServer {
        CodeHostServer {
            id: "bb".to_string(),
            base_url: base_url.to_string(),
            hosts: hosts.iter().map(|host| host.to_string()).collect(),
            trusted_cert_pem: None,
            token_set: false,
        }
    }

    /// The service, its double and the roots its events named.
    fn service(quiet: Duration, building: Duration) -> (Arc<PullRequests>, Arc<Double>, Arc<Mutex<Vec<String>>>) {
        let double = Arc::new(Double::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&events);
        let changed: PullRequestEventSink = Arc::new(move |event| lock(&heard).push(event.root));
        let service = PullRequests::with_ticks(Arc::clone(&double) as Arc<dyn CodeHost>, changed, quiet, building);
        (Arc::new(service), double, events)
    }

    fn connected() -> (Arc<PullRequests>, Arc<Double>, Arc<Mutex<Vec<String>>>) {
        code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
        service(Duration::from_secs(60), Duration::from_secs(60))
    }

    /// Waits up to `limit` for `done`; the watcher's thread answers in its own time.
    fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        done()
    }

    // ---------------------------------------------------------------- reading

    #[test]
    fn the_strip_is_put_together_from_the_server_s_answers() {
        with_app_dir("prs-strip", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            *lock(&double.builds) = vec![build(BuildState::Successful)];
            *lock(&double.comments) = vec![remark(101, false), remark(103, true)];
            *lock(&double.me) = Some("u_me".to_string());

            let PrStrip::Ready { server_id, repo: slug, branch, snapshot, can_approve, approved, merge_blocked, latest_build, open_comments } =
                service.strip(&repo("prs-strip-repo", Some(REMOTE))).unwrap()
            else {
                panic!("not ready")
            };
            assert_eq!((server_id.as_str(), slug.as_str(), branch.as_str()), ("bb", "tax-api", "feature/x"));
            assert_eq!(snapshot.pr.map(|pr| pr.version), Some(6));
            assert!(snapshot.branch_on_server);
            assert_eq!(snapshot.default_branch.as_deref(), Some("master"));
            assert_eq!(snapshot.builds, [build(BuildState::Successful)]);
            assert_eq!(snapshot.comments, [remark(101, false), remark(103, true)]);
            assert_eq!(open_comments, 1, "the resolved thread is not open");
            assert!(can_approve && !approved);
            assert_eq!(merge_blocked, Some(vec!["Requires approvals".to_string()]));
            assert_eq!(latest_build, Some(build(BuildState::Successful)));
        });
    }

    /// The pull request and the builds wait on the branch and not on each
    /// other: in each of its three rounds the reads are asked together, and a
    /// read costs the slower of each round, not the sum of all seven.
    #[test]
    fn a_read_asks_together_what_does_not_wait_on_each_other() {
        with_app_dir("prs-rounds", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            *lock(&double.me) = Some("u_me".to_string());
            // Three rounds of 250 ms are 750; any one round asked in turn is 1250.
            for call in ["default_branch", "branch_commit", "pull_request", "builds", "merge_check", "comments", "me"] {
                lock(&double.slow).insert(call, Duration::from_millis(250));
            }
            let started = Instant::now();
            assert!(matches!(service.strip(&repo("prs-rounds-repo", Some(REMOTE))), Ok(PrStrip::Ready { snapshot: Snapshot { pr: Some(_), .. }, .. })));
            assert!(started.elapsed() < Duration::from_millis(950), "a round asked one after the other: {:?}", started.elapsed());
            assert_eq!((double.asked("builds"), double.asked("merge_check"), double.asked("comments"), double.asked("me")), (1, 1, 1, 1));
        });
    }

    /// The agent's read: the branch it names, or the folder's; the strip's
    /// own state untouched; a server out of reach said rather than hidden; and
    /// the token nowhere in what comes back.
    #[test]
    fn the_agent_reads_any_branch_and_leaves_the_strip_alone() {
        with_app_dir("prs-agent", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k-secret".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_secs(60), Duration::from_secs(60));
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            let root = repo("prs-agent-repo", Some(REMOTE));
            // The strip on screen: its watcher must not take the agent's read as the window's.
            service.watch(root.clone());

            let read = service.status(&root, Some(" release/1.2 ")).unwrap();
            let PrStrip::Ready { branch, .. } = &read else { panic!("{read:?}") };
            assert_eq!(branch, "release/1.2");
            service.status(&root, Some("")).unwrap();
            service.status(&root, None).unwrap();
            let checked_out = crate::infra::git_head::checked_out_branch(&root).unwrap();
            assert_eq!(*lock(&double.branches), ["release/1.2".to_string(), checked_out.clone(), checked_out]);
            assert!(!serde_json::to_string(&read).unwrap().contains("t0k-secret"));

            assert_eq!(service.last(&root), None, "the strip's last read is its own");
            assert!(lock(&events).is_empty(), "nothing for the window to read again");
            assert!(lock(&service.pushed).is_none(), "nor its sense of what was pushed");
            let watched = lock(&service.watching).as_ref().map(|watching| lock(&watching.last).is_none());
            assert_eq!(watched, Some(true), "nor what the watcher compares with");
            service.unwatch(&root);

            *lock(&double.down) = Some(CodeHostError::Unreachable("refused".to_string()));
            assert!(matches!(service.status(&root, None), Err(CodeHostError::Unreachable(_))));
        });
    }

    /// The last read is there for the strip to open with, asking nothing; a
    /// failed one, or one of another folder, is not.
    #[test]
    fn the_last_read_is_handed_back_without_asking() {
        with_app_dir("prs-last", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            let root = repo("prs-last-repo", Some(REMOTE));
            assert_eq!(service.last(&root), None, "nothing read yet");
            let read = service.strip(&root).unwrap();
            let asked = double.total();
            assert_eq!(service.last(&root), Some(read));
            assert_eq!(double.total(), asked, "the server is not asked");
            assert_eq!(service.last(&repo("prs-last-other", Some(REMOTE))), None, "another folder");

            *lock(&double.down) = Some(CodeHostError::Unauthorized);
            service.strip(&root).unwrap_err();
            assert_eq!(service.last(&root), None, "a failed read");
        });
    }

    /// No branch on the server is "push first": no pull request, merge check
    /// or builds are asked for one that cannot have them.
    #[test]
    fn a_branch_the_server_does_not_have_asks_nothing_more() {
        with_app_dir("prs-unpushed", || {
            let (service, double, _) = connected();
            *lock(&double.pr) = Some(pull_request(6));

            let PrStrip::Ready { snapshot, merge_blocked, can_approve, .. } =
                service.strip(&repo("prs-unpushed-repo", Some(REMOTE))).unwrap()
            else {
                panic!("not ready")
            };
            assert!(!snapshot.branch_on_server && snapshot.pr.is_none() && snapshot.merge.is_none());
            assert!(merge_blocked.is_none() && !can_approve);
            for unasked in ["pull_request", "merge_check", "builds", "comments", "me"] {
                assert_eq!(double.asked(unasked), 0, "{unasked}");
            }
        });
    }

    /// A pushed branch with no pull request has nothing to approve: whose the
    /// token is is not asked.
    #[test]
    fn no_pull_request_asks_nobody_s_name() {
        with_app_dir("prs-no-pr", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.me) = Some("u_me".to_string());
            let PrStrip::Ready { snapshot, .. } = service.strip(&repo("prs-no-pr-repo", Some(REMOTE))).unwrap() else {
                panic!("not ready")
            };
            assert!(snapshot.branch_on_server && snapshot.pr.is_none());
            assert_eq!((double.asked("builds"), double.asked("me"), double.asked("comments")), (1, 0, 0));
        });
    }

    /// Only what the server could know is a reason to ask it: a commit is
    /// not, a push or a switch of branch is, and another folder always is.
    #[test]
    fn a_git_change_asks_the_server_only_when_something_was_pushed() {
        with_app_dir("prs-git-changed", || {
            let (service, double, _) = connected();
            let root = repo("prs-git-changed-repo", Some(REMOTE));
            assert!(service.git_changed(&root), "never read");
            service.strip(&root).unwrap();
            assert!(!service.git_changed(&root));
            let twin = repo("prs-git-changed-twin", Some(REMOTE));
            assert!(service.git_changed(&twin), "another folder, though its branch and remote are the same");

            let git = git2::Repository::open(&root).unwrap();
            let head = git.head().unwrap().target().unwrap();
            let tree = git.find_commit(head).unwrap().tree().unwrap();
            let sig = git2::Signature::now("T", "t@example.com").unwrap();
            git.commit(Some("HEAD"), &sig, &sig, "local", &tree, &[&git.find_commit(head).unwrap()]).unwrap();
            assert!(!service.git_changed(&root), "a local commit");

            git.remote_set_url("origin", "ssh://git@git.server.com/corp/other.git").unwrap();
            assert!(service.git_changed(&root), "another remote");
            service.strip(&root).unwrap();

            git.reference("refs/remotes/origin/feature/x", head, true, "push").unwrap();
            assert!(service.git_changed(&root), "a push");
            assert!(service.git_changed(&root), "still, until the window reads");
            service.strip(&root).unwrap();
            assert!(!service.git_changed(&root));
            assert_eq!(double.asked("default_branch"), 3, "asking costs the server nothing");
        });
    }

    /// The author is not offered Approve; their own approval is Unapprove.
    #[test]
    fn approve_follows_who_the_token_is() {
        with_app_dir("prs-approve-who", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            let root = repo("prs-approve-who-repo", Some(REMOTE));

            *lock(&double.pr) = Some(PullRequest { author: "u_me".to_string(), ..pull_request(6) });
            *lock(&double.me) = Some("u_me".to_string());
            let PrStrip::Ready { can_approve, approved, .. } = service.strip(&root).unwrap() else { panic!("not ready") };
            assert!(!can_approve && !approved, "their own pull request");

            let mut theirs = pull_request(6);
            theirs.reviewers[0].status = ReviewStatus::Approved;
            *lock(&double.pr) = Some(theirs);
            let PrStrip::Ready { can_approve, approved, .. } = service.strip(&root).unwrap() else { panic!("not ready") };
            assert!(!can_approve && approved, "approved already: Unapprove");
        });
    }

    /// A new token may be someone else's: the user is asked again.
    #[test]
    fn a_new_token_is_asked_whose_it_is() {
        with_app_dir("prs-me-new-token", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            *lock(&double.me) = Some("u_me".to_string());
            let root = repo("prs-me-new-token-repo", Some(REMOTE));
            service.strip(&root).unwrap();

            service.save(server("https://git.server.com", &[]), Some("other".to_string()), None).unwrap();
            *lock(&double.me) = Some("u_other".to_string());
            service.strip(&root).unwrap();
            assert_eq!(double.asked("me"), 2);
        });
    }

    /// Whose token it is does not change while the token does not: asked once
    /// for the session, not once a read.
    #[test]
    fn who_the_token_belongs_to_is_asked_once() {
        with_app_dir("prs-me-once", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            *lock(&double.me) = Some("u_me".to_string());
            let root = repo("prs-me-once-repo", Some(REMOTE));
            service.strip(&root).unwrap();
            service.strip(&root).unwrap();
            assert_eq!(double.asked("me"), 1);
        });
    }

    /// A server that does not say who the token is still draws the strip —
    /// without Approve.
    #[test]
    fn an_unknown_user_is_no_approve_rather_than_no_strip() {
        with_app_dir("prs-me-unknown", || {
            let (service, double, _) = connected();
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            let PrStrip::Ready { can_approve, .. } = service.strip(&repo("prs-me-unknown-repo", Some(REMOTE))).unwrap() else {
                panic!("not ready")
            };
            assert!(!can_approve);
        });
    }

    /// No remote, a remote nobody added, a detached HEAD: each is a state of
    /// its own, and none of them sends a request — least of all the token to a
    /// host the user never named.
    #[test]
    fn a_folder_with_nothing_to_ask_asks_nothing() {
        with_app_dir("prs-nothing", || {
            let (service, double, _) = connected();
            assert_eq!(service.strip(&repo("prs-no-remote", None)).unwrap(), PrStrip::NoRemote);
            assert_eq!(service.strip(&temp_dir("prs-plain-folder")).unwrap(), PrStrip::NoRemote);

            let other = service.strip(&repo("prs-other-host", Some("https://elsewhere.example/scm/P/r.git"))).unwrap();
            assert!(
                matches!(&other, PrStrip::NotConnected { remote } if remote.base_url == "https://elsewhere.example"),
                "{other:?}"
            );

            let detached = repo("prs-detached", Some(REMOTE));
            let git = git2::Repository::open(&detached).unwrap();
            let head = git.head().unwrap().target().unwrap();
            git.set_head_detached(head).unwrap();
            assert_eq!(service.strip(&detached).unwrap(), PrStrip::NoBranch);

            assert_eq!(double.total(), 0);
        });
    }

    // ---------------------------------------------------------------- buttons

    /// A stale version is refused once and not sent again: the strip is told
    /// to read, and the user sees the new state before deciding again.
    #[test]
    fn a_merge_refused_as_stale_is_not_repeated_and_the_strip_reads_again() {
        with_app_dir("prs-merge-stale", || {
            let (service, double, events) = connected();
            *lock(&double.pr) = Some(pull_request(7));
            *lock(&double.merged) = Some(CodeHostError::Conflict);
            let root = repo("prs-merge-stale-repo", Some(REMOTE));

            assert_eq!(service.merge(&root, 153, 7), Err(CodeHostError::Conflict));
            assert_eq!(double.asked("merge v7"), 1, "the version just read, sent once");
            assert_eq!(*lock(&events), [root.display().to_string()]);
        });
    }

    /// What is merged is what the user saw: a pull request that moved since the
    /// strip showed it, or another one on the branch now, is not sent at all.
    #[test]
    fn a_merge_of_what_the_strip_no_longer_shows_sends_nothing() {
        with_app_dir("prs-merge-moved", || {
            let (service, double, events) = connected();
            *lock(&double.pr) = Some(pull_request(8));
            let root = repo("prs-merge-moved-repo", Some(REMOTE));

            assert_eq!(service.merge(&root, 153, 7), Err(CodeHostError::Conflict), "a newer version");
            assert_eq!(service.merge(&root, 152, 8), Err(CodeHostError::Conflict), "another pull request");
            assert_eq!(double.asked("merge v8"), 0);
            assert_eq!(lock(&events).len(), 2, "the strip reads again");

            service.merge(&root, 153, 8).unwrap();
            assert_eq!(double.asked("merge v8"), 1);
        });
    }

    /// Mark ready and back to a draft change what the user saw, like a merge:
    /// a pull request that moved since is not sent.
    #[test]
    fn a_draft_is_marked_ready_only_as_it_was_seen() {
        with_app_dir("prs-draft", || {
            let (service, double, events) = connected();
            *lock(&double.pr) = Some(pull_request(8));
            let root = repo("prs-draft-repo", Some(REMOTE));

            assert_eq!(service.set_draft(&root, 153, 7, false), Err(CodeHostError::Conflict), "a newer version");
            assert_eq!(service.set_draft(&root, 152, 8, false), Err(CodeHostError::Conflict), "another pull request");
            assert_eq!(double.asked("draft false v8"), 0);
            assert_eq!(lock(&events).len(), 2, "the strip reads again");

            service.set_draft(&root, 153, 8, false).unwrap();
            service.set_draft(&root, 153, 8, true).unwrap();
            assert_eq!((double.asked("draft false v8"), double.asked("draft true v8")), (1, 1));
        });
    }

    #[test]
    fn approve_sends_the_token_s_user_and_a_missing_pull_request_is_said() {
        with_app_dir("prs-approve", || {
            let (service, double, events) = connected();
            *lock(&double.me) = Some("u_me".to_string());
            let root = repo("prs-approve-repo", Some(REMOTE));

            assert_eq!(service.approve(&root, true), Err(CodeHostError::NoPullRequest("feature/x".to_string())));
            *lock(&double.pr) = Some(pull_request(6));
            service.approve(&root, false).unwrap();
            assert_eq!(double.asked("approve u_me false"), 1);
            assert_eq!(lock(&events).len(), 2, "a refusal is news too");
        });
    }

    #[test]
    fn create_aims_at_the_default_branch_and_needs_the_branch_pushed() {
        with_app_dir("prs-create", || {
            let (service, double, events) = connected();
            let root = repo("prs-create-repo", Some(REMOTE));

            assert_eq!(service.create(&root, "t", "", None, false).unwrap_err(), CodeHostError::NotPushed("feature/x".to_string()));
            assert_eq!(double.asked("create feature/x -> master: t"), 0);
            assert_eq!(lock(&events).len(), 1, "the strip reads again after a refusal too");

            *lock(&double.commit) = Some("94beed9".to_string());
            service.create(&root, "  ", "", None, false).unwrap();
            assert_eq!(double.asked("create feature/x -> master: feature/x"), 1, "an empty title is the branch");
            service.create(&root, "t", "", Some("  "), false).unwrap();
            assert_eq!(double.asked("create feature/x -> master: t"), 1, "an empty target is the default branch");
            service.create(&root, "Fix it", "", Some("release"), false).unwrap();
            assert_eq!(double.asked("create feature/x -> release: Fix it"), 1);
            service.create(&root, "Full", "", Some(" refs/heads/release "), false).unwrap();
            assert_eq!(double.asked("create feature/x -> release: Full"), 1, "a full ref is its branch");
            service.create(&root, "Not yet", "", None, true).unwrap();
            assert_eq!(double.asked("create feature/x -> master: Not yet (draft)"), 1, "a draft is asked for as one");
            // The agent's description goes as written; the strip sends none.
            AgentPullRequests::create(service.as_ref(), &root, "Described", "Why:\n- because", None, false).unwrap();
            assert_eq!(double.asked("create feature/x -> master: Described [Why:\n- because]"), 1);
            assert_eq!(lock(&events).len(), 7, "each create says the strip changed");
        });
    }

    /// Each folder a button cannot act in says why, in its own words, and
    /// sends nothing.
    #[test]
    fn a_button_in_a_folder_without_a_target_sends_nothing() {
        with_app_dir("prs-button-nowhere", || {
            let (service, double, events) = connected();
            let elsewhere = repo("prs-button-nowhere-repo", Some("ssh://git@elsewhere.example/P/r.git"));
            assert_eq!(service.merge(&elsewhere, 153, 1), Err(CodeHostError::NoServer("elsewhere.example".to_string())));
            assert_eq!(service.approve(&repo("prs-button-no-remote", None), true), Err(CodeHostError::NoRemote));

            let detached = repo("prs-button-detached", Some(REMOTE));
            let git = git2::Repository::open(&detached).unwrap();
            git.set_head_detached(git.head().unwrap().target().unwrap()).unwrap();
            assert_eq!(service.create(&detached, "t", "", None, false), Err(CodeHostError::NoBranch));

            assert_eq!((double.total(), lock(&events).len()), (0, 0));
        });
    }

    // --------------------------------------------------------------- servers

    /// The ssh host differs from the web one: the server learns it as an
    /// alias, but only once its clone links have shown the repository there.
    #[test]
    fn an_alias_is_learnt_only_when_a_clone_link_proves_the_repository() {
        with_app_dir("prs-alias", || {
            let (service, double, events) = service(Duration::from_secs(60), Duration::from_secs(60));
            let root = repo("prs-alias-repo", Some(REMOTE));
            let web = server("https://bitbucket.server.com", &[]);
            // On screen as well: the folder is told once, not once per reason.
            service.watch(root.clone());

            *lock(&double.links) = vec!["https://bitbucket.server.com/scm/corp/other.git".to_string()];
            let refused = service.save(web.clone(), Some("t0k".to_string()), Some(&root));
            assert_eq!(refused, Err(CodeHostError::NotOnServer("git.server.com/corp/tax-api".to_string())));
            assert!(code_host_store::list().unwrap().is_empty(), "no proof, nothing kept");
            assert!(lock(&events).is_empty());

            *lock(&double.links) = vec![
                "https://bitbucket.server.com/scm/corp/tax-api.git".to_string(),
                "ssh://git@git.server.com:7999/corp/tax-api.git".to_string(),
            ];
            assert_eq!(service.save(web.clone(), None, Some(&root)), Err(CodeHostError::NoToken(web.base_url.clone())));
            let saved = service.save(web.clone(), Some("t0k".to_string()), Some(&root)).unwrap();
            assert_eq!(saved[0].hosts, ["git.server.com"]);
            assert!(saved[0].token_set);
            assert_eq!(*lock(&events), [root.display().to_string()]);

            // Saved again with the kept token, it proves with that one and
            // learns nothing twice.
            let again = service.save(web, None, Some(&root)).unwrap();
            assert_eq!((again.len(), again[0].hosts.as_slice()), (1, ["git.server.com".to_string()].as_slice()));
            assert_eq!(double.asked("clone_links t0k"), 3, "the kept token is the one proven with");
            service.unwatch(&root);

            // And the folder now finds its server by that alias.
            assert!(matches!(service.strip(&root), Ok(PrStrip::Ready { .. })));
        });
    }

    /// A new server gets an id; an address is kept as one, without the slash
    /// typed after it; what is not one is refused before it is kept.
    #[test]
    fn a_saved_server_is_an_address_with_an_id() {
        with_app_dir("prs-save-checked", || {
            let (service, _, _) = service(Duration::from_secs(60), Duration::from_secs(60));
            let fresh = CodeHostServer { id: " ".to_string(), ..server(" https://Bitbucket.server.com/ ", &[]) };
            let saved = service.save(fresh, None, None).unwrap();
            assert_eq!(saved[0].base_url, "https://Bitbucket.server.com");
            assert!(saved[0].id.len() > 8, "{}", saved[0].id);

            let again = service.save(saved[0].clone(), None, None).unwrap();
            assert_eq!(again.len(), 1, "the id it was given is the one it keeps");

            for bad in ["", "bitbucket.server.com", "ftp://bitbucket.server.com", "https://", "https:///x"] {
                let refused = service.save(server(bad, &[]), None, None);
                assert_eq!(refused, Err(CodeHostError::BadAddress(bad.to_string())), "{bad}");
            }
            assert_eq!(code_host_store::list().unwrap().len(), 1);
        });
    }

    /// A save from the feature's panel in Catalog is for no folder: nothing is proven or asked.
    #[test]
    fn a_save_for_no_folder_asks_the_server_nothing() {
        with_app_dir("prs-save-plain", || {
            let (service, double, _) = service(Duration::from_secs(60), Duration::from_secs(60));
            service.save(server("https://bitbucket.server.com", &[]), Some("t".to_string()), None).unwrap();
            assert_eq!(double.total(), 0);
        });
    }

    /// Aliases are learnt from clone links, never taken from the form: a host
    /// typed there would send the token wherever it names.
    #[test]
    fn a_host_from_the_form_is_not_kept() {
        with_app_dir("prs-save-hosts", || {
            let (service, _, _) = service(Duration::from_secs(60), Duration::from_secs(60));
            code_host_store::put(server("https://bitbucket.server.com", &["git.server.com"]), Some("t".to_string())).unwrap();
            let saved = service.save(server("https://bitbucket.server.com", &["evil.example"]), None, None).unwrap();
            assert_eq!(saved[0].hosts, ["git.server.com"], "an edit keeps the learnt ones");

            let fresh = CodeHostServer { id: String::new(), ..server("https://other.server.com", &["evil.example"]) };
            let saved = service.save(fresh, None, None).unwrap();
            assert!(saved.iter().all(|saved| !saved.hosts.contains(&"evil.example".to_string())), "{saved:?}");
        });
    }

    /// A server added or removed in Catalog turns the strip on screen from
    /// NotConnected to Ready and back: the watched folder is told.
    #[test]
    fn a_server_saved_or_deleted_tells_the_watched_folder() {
        with_app_dir("prs-save-watched", || {
            let (service, _, events) = service(Duration::from_secs(60), Duration::from_secs(60));
            let root = temp_dir("prs-save-watched-repo");
            service.save(server("https://git.server.com", &[]), Some("t".to_string()), None).unwrap();
            assert!(lock(&events).is_empty(), "nothing on screen");

            service.watch(root.clone());
            service.save(server("https://git.server.com", &[]), None, None).unwrap();
            let deleted = service.delete("bb").unwrap();
            assert!(deleted.is_empty());
            assert_eq!(*lock(&events), [root.display().to_string(), root.display().to_string()]);
            service.unwatch(&root);
        });
    }

    // --------------------------------------------------------------- watching

    /// The watcher's whole contract: an unchanged server says nothing, a change
    /// is said once, and after unwatch nothing is asked.
    #[test]
    fn the_watcher_speaks_only_of_a_change_and_stops_when_told() {
        with_app_dir("prs-watch", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(20), Duration::from_millis(20));
            let root = repo("prs-watch-repo", Some(REMOTE));

            service.strip(&root).unwrap();
            service.watch(root.clone());
            assert!(eventually(Duration::from_secs(5), || double.asked("default_branch") >= 4), "it ticks");
            assert!(lock(&events).is_empty(), "the window already has this: {:?}", lock(&events));

            *lock(&double.commit) = Some("94beed9".to_string());
            assert!(eventually(Duration::from_secs(5), || !lock(&events).is_empty()), "a pushed branch is a change");
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(*lock(&events), [root.display().to_string()], "said once");

            service.unwatch(&root);
            std::thread::sleep(Duration::from_millis(50));
            let after = double.total();
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(double.total(), after, "asked after unwatch");
        });
    }

    /// A remark arriving and a thread being resolved are changes the window
    /// is told of — the Address comments button lives on them.
    #[test]
    fn a_new_remark_and_a_resolution_are_changes() {
        with_app_dir("prs-watch-remarks", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(20), Duration::from_millis(20));
            *lock(&double.commit) = Some("94beed9".to_string());
            *lock(&double.pr) = Some(pull_request(6));
            let root = repo("prs-watch-remarks-repo", Some(REMOTE));

            service.strip(&root).unwrap();
            service.watch(root.clone());
            assert!(eventually(Duration::from_secs(5), || double.asked("comments") >= 3), "it ticks");
            assert!(lock(&events).is_empty(), "no remarks yet is what the window has");

            *lock(&double.comments) = vec![remark(101, false)];
            assert!(eventually(Duration::from_secs(5), || !lock(&events).is_empty()), "a new remark is a change");
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(*lock(&events), [root.display().to_string()], "said once");

            *lock(&double.comments) = vec![remark(101, true)];
            assert!(eventually(Duration::from_secs(5), || lock(&events).len() >= 2), "a resolution is a change too");
            service.unwatch(&root);
        });
    }

    /// What the window read itself is what it has: the watcher's next read of
    /// the same answer is not news, whoever read first.
    #[test]
    fn the_window_s_own_read_is_what_the_watcher_compares_against() {
        with_app_dir("prs-watch-window", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(30), Duration::from_millis(30));
            let root = repo("prs-watch-window-repo", Some(REMOTE));
            service.watch(root.clone());
            // The watcher's first read is news to a window that has read nothing.
            assert!(eventually(Duration::from_secs(5), || lock(&events).len() == 1));

            *lock(&double.commit) = Some("94beed9".to_string());
            service.strip(&root).unwrap();
            let ticks = double.asked("default_branch");
            assert!(eventually(Duration::from_secs(5), || double.asked("default_branch") >= ticks + 3));
            assert_eq!(lock(&events).len(), 1, "the window read the change itself");
            service.unwatch(&root);
        });
    }

    /// The window's read is a read: the watcher waits a full tick after it
    /// rather than asking the same thing again at once.
    #[test]
    fn the_window_s_read_puts_the_next_tick_off() {
        with_app_dir("prs-watch-wake", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, _) = service(Duration::from_secs(60), Duration::from_secs(60));
            let root = repo("prs-watch-wake-repo", Some(REMOTE));
            service.watch(root.clone());
            service.strip(&root).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(double.asked("default_branch"), 1, "only the window's own read");
            service.unwatch(&root);
        });
    }

    /// Watching the folder already watched keeps what the watcher has seen:
    /// a change already said is not said again.
    #[test]
    fn watching_again_is_not_starting_over() {
        with_app_dir("prs-watch-again", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(20), Duration::from_millis(20));
            let root = repo("prs-watch-again-repo", Some(REMOTE));
            service.strip(&root).unwrap();
            service.watch(root.clone());
            *lock(&double.commit) = Some("94beed9".to_string());
            assert!(eventually(Duration::from_secs(5), || lock(&events).len() == 1));

            service.watch(root.clone());
            let ticks = double.asked("default_branch");
            assert!(eventually(Duration::from_secs(5), || double.asked("default_branch") >= ticks + 3));
            assert_eq!(lock(&events).len(), 1, "the change was said already");
            service.unwatch(&root);
        });
    }

    /// An answer that arrives after the strip went away is told to nobody.
    #[test]
    fn a_read_that_ends_after_unwatch_is_not_reported() {
        with_app_dir("prs-watch-late", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(20), Duration::from_millis(20));
            let root = repo("prs-watch-late-repo", Some(REMOTE));
            service.strip(&root).unwrap();
            lock(&double.slow).insert("builds", Duration::from_millis(300));
            *lock(&double.commit) = Some("94beed9".to_string());
            service.watch(root.clone());
            assert!(eventually(Duration::from_secs(5), || double.asked("default_branch") == 2), "a read in flight");
            service.unwatch(&root);
            std::thread::sleep(Duration::from_millis(500));
            assert!(lock(&events).is_empty(), "{:?}", lock(&events));
        });
    }

    /// A server that stops answering is a change too: the strip goes quiet,
    /// and is told once.
    #[test]
    fn a_server_going_down_is_reported_once() {
        with_app_dir("prs-watch-down", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(20), Duration::from_millis(20));
            let root = repo("prs-watch-down-repo", Some(REMOTE));
            assert!(service.strip(&root).is_ok());
            service.watch(root.clone());

            *lock(&double.down) = Some(CodeHostError::Unreachable("refused".to_string()));
            assert!(eventually(Duration::from_secs(5), || !lock(&events).is_empty()));
            let ticks = double.asked("default_branch");
            assert!(eventually(Duration::from_secs(5), || double.asked("default_branch") >= ticks + 3));
            assert_eq!(lock(&events).len(), 1, "still down is not news");
            assert_eq!(service.strip(&root), Ok(PrStrip::Unreachable));
            // A server that answers and refuses is something to say: the user can act on it.
            *lock(&double.down) = Some(CodeHostError::Unauthorized);
            assert_eq!(service.strip(&root), Err(CodeHostError::Unauthorized));
            // So is a connection that reached it and broke — a TLS alert, a garbled answer.
            *lock(&double.down) = Some(CodeHostError::Network("handshake alert".to_string()));
            assert_eq!(service.strip(&root), Err(CodeHostError::Network("handshake alert".to_string())));
            service.unwatch(&root);
        });
    }

    /// The window read while the watcher's own read was in flight: the window
    /// has the newer answer, and the watcher's older one is not news.
    #[test]
    fn a_read_overtaken_by_the_window_s_is_not_reported() {
        with_app_dir("prs-watch-overtaken", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, events) = service(Duration::from_millis(50), Duration::from_millis(50));
            *lock(&double.commit) = Some("94beed9".to_string());
            let root = repo("prs-watch-overtaken-repo", Some(REMOTE));
            service.strip(&root).unwrap();

            lock(&double.slow).insert("builds", Duration::from_millis(300));
            service.watch(root.clone());
            assert!(eventually(Duration::from_secs(5), || double.asked("builds") == 2), "the watcher's read in flight");
            lock(&double.slow).clear();
            *lock(&double.pr) = Some(pull_request(6));
            assert!(matches!(service.strip(&root), Ok(PrStrip::Ready { snapshot: Snapshot { pr: Some(_), .. }, .. })));

            std::thread::sleep(Duration::from_millis(500));
            assert!(lock(&events).is_empty(), "{:?}", lock(&events));
            service.unwatch(&root);
        });
    }

    /// A running build is when the strip is looked at: the watcher asks sooner.
    #[test]
    fn a_running_build_brings_the_next_read_closer() {
        with_app_dir("prs-watch-building", || {
            code_host_store::put(server("https://git.server.com", &[]), Some("t0k".to_string())).unwrap();
            let (service, double, _) = service(Duration::from_secs(60), Duration::from_millis(20));
            *lock(&double.commit) = Some("94beed9".to_string());
            let root = repo("prs-watch-building-repo", Some(REMOTE));

            service.strip(&root).unwrap();
            service.watch(root.clone());
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(double.asked("default_branch"), 1, "nothing running: the quiet tick");
            service.unwatch(&root);

            *lock(&double.builds) = vec![build(BuildState::InProgress)];
            service.watch(root.clone());
            service.strip(&root).unwrap();
            assert!(eventually(Duration::from_secs(5), || double.asked("default_branch") >= 5), "the building tick");
            service.unwatch(&root);
        });
    }

    /// An unwatch from the folder just left can come after the next folder's
    /// watch; it must not stop that one.
    #[test]
    fn unwatching_another_folder_leaves_the_watch_alone() {
        with_app_dir("prs-watch-other", || {
            let (service, _, _) = service(Duration::from_secs(60), Duration::from_secs(60));
            let root = temp_dir("prs-watch-other-a");
            service.watch(root.clone());
            service.unwatch(&temp_dir("prs-watch-other-b"));
            assert!(lock(&service.watching).is_some());
            service.unwatch(&root);
            assert!(lock(&service.watching).is_none());
        });
    }

    #[test]
    fn identify_asks_the_host() {
        let (service, double, _) = service(Duration::from_secs(60), Duration::from_secs(60));
        assert_eq!(service.identify(&server("https://x", &[])).unwrap(), "Bitbucket 8.19.26");
        assert_eq!(double.asked("identify"), 1);
    }
}
