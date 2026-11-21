//! A pull request for the open folder's branch, in words that name no host.
//!
//! The Bitbucket Data Center adapter is `infra::bitbucket_client` and the JSON
//! it reads stops there; what a strip needs from any host is here: where the
//! repository sits on a server, which configured server it belongs to, what a
//! snapshot holds, and what that snapshot allows. Decided by the probe recorded
//! in `docs/56-bitbucket-prs.md`: the forms a remote comes in, `/scm/` and the
//! case of a project key, and the veto lines a merge check returns ready-made.
//!
//! **A remote is read to fill a form, never trusted.** Nothing here decides that
//! an address belongs to Bitbucket: the server list is the source of truth, and
//! a remote only says which of the configured servers to look at (`server_for`)
//! and where a form should put its own guess at a base address (`base_url`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CodeHostError {
    /// Not an address this app can read a project and a repository out of. The
    /// caller shows the base address with empty project and slug fields rather
    /// than inventing a guess.
    #[error("{0} does not name a project and a repository on a server")]
    UnreadableRemote(String),
    /// The server answered, and not with a yes: the status and its own words,
    /// cut short. The variants after it are the ones the strip acts on.
    #[error("the server answered {0}: {1}")]
    Http(u16, String),
    #[error("the token was refused — it may have expired or been revoked; a new one goes in Catalog → Pull requests")]
    Unauthorized,
    #[error("the token may read but not do this — merge and create need Repository write")]
    Forbidden,
    /// The optimistic lock of [`PullRequest::version`]: someone else changed the
    /// pull request first, and the strip reads it again rather than repeating
    /// what it sent.
    #[error("the pull request changed while this was sent — it was read again")]
    Conflict,
    /// The app trusts the public roots or the PEM a server carries, never a
    /// keychain, so a corporate server says this on the first request.
    #[error("the server's certificate is not trusted — the server in Catalog → Pull requests needs its CA as a PEM: {0}")]
    Tls(String),
    #[error("the server's answer is not what was asked for: {0}")]
    BadAnswer(String),
    #[error("{0}")]
    Network(String),
    /// No route to the server at all — its name does not resolve, nothing
    /// listens, the connection times out: off the VPN, as a rule. The strip
    /// shows only the branch then. Anything the server itself said back (a TLS
    /// alert, a broken answer) is [`CodeHostError::Network`] and is shown.
    #[error("the server is out of reach: {0}")]
    Unreachable(String),
    #[error("no server is configured for {0} — add one in Catalog → Pull requests")]
    NoServer(String),
    /// The server is there and has no token yet: telling the user to add the
    /// server again is how a form loses them.
    #[error("the server \"{0}\" has no token — add one in Catalog → Pull requests")]
    NoToken(String),
    /// A token goes over TLS or nowhere; `http` is allowed only to this machine,
    /// where a test's own server listens.
    #[error("{0} is not https — a token goes over TLS or nowhere; fix the address in Catalog → Pull requests")]
    Insecure(String),
    #[error("{0}")]
    Store(String),
    /// The server answered for the repository and none of its clone links is
    /// the folder's remote: it is some other server's, and no alias is learnt.
    #[error("the server does not list {0} among its clone links — it is not this repository's server")]
    NotOnServer(String),
    #[error("this folder has no remote a server could be found by")]
    NoRemote,
    #[error("the branch {0} has no open pull request")]
    NoPullRequest(String),
    #[error("the branch {0} is not on the server — push it first")]
    NotPushed(String),
    #[error("{0} is not a server address — it starts with https:// and names a host")]
    BadAddress(String),
    /// A detached HEAD, or a folder that is no repository: there is no branch
    /// for a pull request to come from.
    #[error("no branch is checked out")]
    NoBranch,
}

fn unreadable(url: &str) -> CodeHostError {
    CodeHostError::UnreadableRemote(without_credentials(url))
}

/// What an error may say about an address: all of it but a credential inside it.
/// A token in a clone URL is ordinary — CI writes them — and this text reaches
/// the window and, through the agent's tool, a call log that redacts the tools it
/// knows and nothing else.
fn without_credentials(url: &str) -> String {
    let trimmed = url.trim();
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), trimmed),
    };
    let (authority, path) = split_authority(rest);
    format!("{scheme}{}{path}", without_userinfo(authority))
}

/// What follows the scheme, cut where the authority ends: the first `/`. A
/// `?` or `#` before it ends it too.
fn split_authority(rest: &str) -> (&str, &str) {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest.split_at(end)
}

/// The authority without `user:password@`. The last `@` is the one that ends
/// it: a password may carry one of its own, and splitting at the first would
/// leave the rest of the password in front of the host.
fn without_userinfo(authority: &str) -> &str {
    authority.rsplit_once('@').map_or(authority, |(_, host)| host)
}

/// How a remote reaches the server. It decides what the address can say about
/// the web one: a clone over HTTP carries the context path and the port, an ssh
/// clone carries neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    /// `http://` or `https://`. The address it implies is `https://` either way —
    /// see [`RemoteRepo::parse`].
    Web,
    Ssh,
}

/// A remote address read into what a Bitbucket-shaped server needs.
///
/// Three forms reach one here — `https://host[/context]/scm/PROJECT/repo.git`,
/// `ssh://git@host[:port]/PROJECT/repo.git` and git's own
/// `git@host:PROJECT/repo.git` — and a personal repository stands where the
/// project does, as `~user` (Data Center's spelling for one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    /// Lowercase, no port: what a configured server is matched by.
    pub host: String,
    /// The web address this remote implies, for the connect form to offer:
    /// `https://host[:port][/context]` from an https clone, and `https://host`
    /// from an ssh one.
    pub base_url: String,
    /// `corp-wowtax`, or `~user` for a personal repository.
    pub project: String,
    pub slug: String,
    pub transport: Transport,
}

impl RemoteRepo {
    pub fn parse(url: &str) -> Result<Self, CodeHostError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(unreadable(url));
        }

        let (transport, authority, path) = if let Some(at) = trimmed.find("://") {
            let scheme = trimmed[..at].to_ascii_lowercase();
            let transport = match scheme.as_str() {
                "http" | "https" => Transport::Web,
                "ssh" => Transport::Ssh,
                _ => return Err(unreadable(url)),
            };
            // A token in a clone URL is a real thing people have; it names
            // nobody and is quietly dropped.
            let (authority, path) = split_authority(&trimmed[at + 3..]);
            (transport, without_userinfo(authority), path.trim_start_matches('/'))
        } else {
            // `user@host:project/repository`.
            let (user_host, path) = trimmed.split_once(':').ok_or_else(|| unreadable(url))?;
            (Transport::Ssh, without_userinfo(user_host), path)
        };

        let (host, port) = split_port(authority).ok_or_else(|| unreadable(url))?;
        let host = host.to_ascii_lowercase();
        if host.is_empty() {
            return Err(unreadable(url));
        }

        let path = path.trim_end_matches('/');
        // Stripped in any case, as `scm` is matched: a remote written `repo.GIT`
        // would otherwise name a second repository beside `repo`.
        let path = if path.to_ascii_lowercase().ends_with(".git") { &path[..path.len() - 4] } else { path };
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();

        // An https clone of Data Center is `/scm/PROJECT/repo`; without `/scm/`
        // it is some other host's layout, and reading two arbitrary segments out
        // of it is how `https://` itself becomes a host name.
        let (context, repo) = match transport {
            Transport::Web => {
                let at = segments
                    .iter()
                    .position(|part| part.eq_ignore_ascii_case("scm"))
                    .ok_or_else(|| unreadable(url))?;
                (segments[..at].join("/"), &segments[at + 1..])
            }
            Transport::Ssh => (String::new(), &segments[..]),
        };

        let (project, slug) = match repo {
            [project, slug] => (*project, *slug),
            _ => return Err(unreadable(url)),
        };

        let base_url = match transport {
            // Always `https://`, even from an `http://` clone: a bearer token goes
            // over TLS or nowhere, so the form never proposes a channel it may not
            // send one down.
            Transport::Web => {
                let port = port.map(|port| format!(":{port}")).unwrap_or_default();
                let context = if context.is_empty() { String::new() } else { format!("/{context}") };
                format!("https://{host}{port}{context}")
            }
            // An ssh port is the ssh port: Data Center answers 22 or 7999 there
            // and leaves the web address alone, which the probe saw as 22 and 443.
            Transport::Ssh => format!("https://{host}"),
        };

        Ok(Self {
            host,
            base_url,
            project: project.to_string(),
            slug: slug.to_string(),
            transport,
        })
    }

    /// What decides whether two addresses name one repository. Drops the scheme,
    /// the context path, `/scm/` and any credentials; lowercases the host and the
    /// project key, because a key is case-insensitive and its web spelling is
    /// upper case where `/scm/` and the API use lower. The slug keeps its case.
    pub fn canonical(&self) -> String {
        format!("{}/{}/{}", self.host, self.project.to_lowercase(), self.slug)
    }
}

/// The canonical form of any address that names a repository, so that a clone
/// link the server lists and the folder's own `origin` compare equal.
pub fn canonical_remote(url: &str) -> Result<String, CodeHostError> {
    RemoteRepo::parse(url).map(|remote| remote.canonical())
}

/// `host:port` → host and port; a port that is not one is not an address.
fn split_port(authority: &str) -> Option<(&str, Option<u16>)> {
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host, Some(port.parse::<u16>().ok()?))),
        None => Some((authority, None)),
    }
}

/// A server the user added: the address every request goes to, the certificate
/// to trust instead of the public roots, and the hosts it is also known by.
///
/// The token itself is never here — sealed like a provider's key, with only
/// [`CodeHostServer::token_set`] in the window's hands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodeHostServer {
    pub id: String,
    pub base_url: String,
    /// Hosts this server answers to besides its own — an ssh host that differs
    /// from the web one. `AGENTS.md`'s rule for remembering one: only after a
    /// clone link has proved the repository is there.
    pub hosts: Vec<String>,
    /// A certificate authority to trust *instead of* the public roots, for a
    /// server behind a corporate CA — the same field a provider has.
    pub trusted_cert_pem: Option<String>,
    pub token_set: bool,
}

/// The host of a base address, lowercase and without the port; an IPv6
/// literal without its brackets.
pub fn host_of(base_url: &str) -> Option<String> {
    let rest = base_url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, after)| after);
    let authority = without_userinfo(split_authority(rest).0);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// The server a remote belongs to: the one whose own address names that host,
/// and failing that one that has met it before.
///
/// No match is `None`, deliberately: falling back to the first configured server
/// would send a token to a host the user never pointed it at.
pub fn server_for<'a>(servers: &'a [CodeHostServer], remote: &RemoteRepo) -> Option<&'a CodeHostServer> {
    // The address the remote itself implies comes first: two servers may share a
    // host and differ by port or context path, and only one of them is this.
    single(servers.iter().filter(|server| same_base(&server.base_url, &remote.base_url)))
        // An ssh remote names no context path and no web port, so two servers
        // answering to one host is a question for the user, not a guess: a token
        // sent to the wrong instance is what decision 1 forbids.
        .or_else(|| single(servers.iter().filter(|server| serves_host(server, &remote.host))))
        .or_else(|| single(servers.iter().filter(|server| knows_host(server, &remote.host))))
}

/// Whether the server's own address names this host.
fn serves_host(server: &CodeHostServer, host: &str) -> bool {
    host_of(&server.base_url).as_deref() == Some(host)
}

/// Whether the server answers to this host as a remembered alias.
fn knows_host(server: &CodeHostServer, host: &str) -> bool {
    server.hosts.iter().any(|known| known.eq_ignore_ascii_case(host))
}

/// The one item, or `None` when there is none — or when several match and only a
/// guess could pick between them.
fn single<'a, I: Iterator<Item = &'a CodeHostServer>>(mut items: I) -> Option<&'a CodeHostServer> {
    let first = items.next()?;
    if items.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// Whether two base addresses are one address, as a user may write it: a trailing
/// slash and the case of the host make no difference.
fn same_base(one: &str, other: &str) -> bool {
    let plain = |address: &str| address.trim().trim_end_matches('/').to_ascii_lowercase();
    !one.trim().is_empty() && plain(one) == plain(other)
}

/// The alias to remember for a remote's host, when the server does not already
/// answer to it. The caller remembers it only after the server has listed this
/// repository among its clone links — a host that merely names a server is not
/// proof that it is that server.
pub fn alias_to_remember(server: &CodeHostServer, remote: &RemoteRepo) -> Option<String> {
    let known = serves_host(server, &remote.host) || knows_host(server, &remote.host);
    if known {
        None
    } else {
        Some(remote.host.clone())
    }
}

// ------------------------------------------------------------- what is asked

/// One request's worth of what a server needs: where it is, the repository in
/// it, and the token. Not `Serialize` — the token is in here, and only
/// `infra::code_host_store` ever reads it back off the disk.
pub struct Call<'a> {
    pub server: &'a CodeHostServer,
    pub token: &'a str,
    pub remote: &'a RemoteRepo,
}

/// What a strip asks of a server, in words that name no host: the reads the
/// snapshot is assembled from, and the three actions its buttons send.
/// `infra::bitbucket_client` implements it; a test's own double stands in for
/// it where the service is tested (BB-3).
pub trait CodeHost: Send + Sync {
    /// What the server says it is, asked without a token — the connect form's
    /// check that the address is this kind of server at all. `Err` when it
    /// answers as something else.
    fn identify(&self, server: &CodeHostServer) -> Result<String, CodeHostError>;
    /// The repository's clone links — the membership check of decision 1, which
    /// the domain does with [`canonical_remote`], not this.
    fn clone_links(&self, call: &Call) -> Result<Vec<String>, CodeHostError>;
    /// The slug of the user the token belongs to.
    fn me(&self, call: &Call) -> Result<String, CodeHostError>;
    fn default_branch(&self, call: &Call) -> Result<String, CodeHostError>;
    /// The commit the branch points at, when the branch is on the server at
    /// all — `None` is exactly the "push the branch first" of decision 5.
    fn branch_commit(&self, call: &Call, branch: &str) -> Result<Option<String>, CodeHostError>;
    fn pull_request(&self, call: &Call, branch: &str) -> Result<Option<PullRequest>, CodeHostError>;
    fn merge_check(&self, call: &Call, id: u64) -> Result<MergeCheck, CodeHostError>;
    fn builds(&self, call: &Call, commit: &str) -> Result<Vec<Build>, CodeHostError>;
    /// `false` takes an approval back; `me` is the reviewer's slug, from [`CodeHost::me`].
    fn approve(&self, call: &Call, pr: &PullRequest, me: &str, approved: bool) -> Result<(), CodeHostError>;
    fn merge(&self, call: &Call, pr: &PullRequest) -> Result<(), CodeHostError>;
    /// `false` marks a draft ready for review; `true` takes it back to a draft.
    /// At `pr.version`, as merge is: what changes is what was seen.
    fn set_draft(&self, call: &Call, pr: &PullRequest, draft: bool) -> Result<(), CodeHostError>;
    /// An empty `description` sends none.
    fn create(&self, call: &Call, from: &str, to: &str, title: &str, description: &str, draft: bool) -> Result<PullRequest, CodeHostError>;
}

// ------------------------------------------------------------------ the strip

/// What a reviewer has said about a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewStatus {
    Approved,
    Unapproved,
    NeedsWork,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    /// Who the server means: what the token's user is compared with.
    pub slug: String,
    /// What a person reads — "Ivan Petrov" for `u_m2grr`; the slug when the
    /// server gives none.
    pub display_name: String,
    pub status: ReviewStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub id: u64,
    pub title: String,
    /// The server's optimistic lock: a merge or an approve sent with a stale one
    /// is refused with 409, and the strip re-reads instead of repeating.
    pub version: u64,
    /// The reviewer's slug, so that the author is recognised as one.
    pub author: String,
    /// What a person reads; the slug when the server gives none.
    pub author_name: String,
    pub from_branch: String,
    pub to_branch: String,
    pub reviewers: Vec<Reviewer>,
    /// The page Open opens.
    pub url: String,
    /// Work in progress (Data Center 8.18 and later): reviewers see it, the
    /// server merges nothing until it is marked ready. `None` is a server
    /// before drafts — it never names the field, and has nothing to convert to.
    pub draft: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildState {
    Successful,
    Failed,
    InProgress,
    /// A state this app has no word for — shown as it is, never as success.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub key: String,
    pub name: String,
    pub state: BuildState,
    pub url: String,
    /// When the server took this status, in epoch milliseconds; a server
    /// that does not say leaves it out.
    pub date_added: Option<u64>,
}

/// The build the strip shows: the newest the server took — a run started
/// after one that finished is the one that matters. Undated ones are older
/// than any dated; among equals a running one wins, then the server's last.
pub fn latest_build(builds: &[Build]) -> Option<&Build> {
    builds.iter().max_by_key(|build| (build.date_added, build.state == BuildState::InProgress))
}

/// The server's answer to "may this be merged".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeCheck {
    pub can_merge: bool,
    pub conflicted: bool,
    /// The server's own lines, shown as they come: it knows why better than this
    /// app does, and they are the same words its web page shows.
    pub vetoes: Vec<String>,
}

/// Everything the strip draws, in one comparison.
///
/// The watcher sends its event only when a snapshot is `!=` the last one, so
/// this type carries what is drawn and nothing else — a timestamp here would
/// make every poll a change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    pub pr: Option<PullRequest>,
    /// Whether the branch is on the server at all: without it Create PR has
    /// nothing to offer and the strip says to push first.
    pub branch_on_server: bool,
    /// Where Create PR aims unless the user picks another branch.
    pub default_branch: Option<String>,
    /// `None` until the strip has asked, which is not the same as "no".
    pub merge: Option<MergeCheck>,
    pub builds: Vec<Build>,
}

/// What the strip shows for the open folder, from nothing to a pull request.
///
/// Compared whole by the watcher, like [`Snapshot`]: what is in here is what
/// is drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum PrStrip {
    /// No remote, or not a repository: nothing to show.
    NoRemote,
    /// A detached HEAD: no branch for a pull request.
    NoBranch,
    /// The server could not be reached — no route, no name, no answer in
    /// time: nothing to show but the branch. A server that answered and
    /// refused is an error instead, which the user can act on.
    Unreachable,
    /// A remote no configured server answers to: the connect form, filled
    /// from what the remote says.
    #[serde(rename_all = "camelCase")]
    NotConnected { remote: RemoteRepo },
    #[serde(rename_all = "camelCase")]
    Ready {
        server_id: String,
        /// The repository's slug, for the line under the title: "tax-api #153".
        repo: String,
        branch: String,
        snapshot: Snapshot,
        /// [`can_approve`] for the token's user.
        can_approve: bool,
        /// The token's user has approved: Unapprove is offered instead.
        approved: bool,
        /// [`merge_blocked`]: `None` with a pull request is a live Merge.
        merge_blocked: Option<Vec<String>>,
        /// [`latest_build`] of the snapshot's builds.
        latest_build: Option<Build>,
    },
}

impl PrStrip {
    /// Whether the latest build is running — the watcher asks sooner while
    /// it is. An older status left "in progress" by a run that died does not
    /// keep it asking.
    pub fn building(&self) -> bool {
        matches!(self, PrStrip::Ready { latest_build: Some(build), .. } if build.state == BuildState::InProgress)
    }
}

/// The strip of a folder changed: the window reads it again. `root` is the
/// folder as `workspace_open` named it — an event outlives its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestChanged {
    pub root: String,
}

pub type PullRequestEventSink = Arc<dyn Fn(PullRequestChanged) + Send + Sync>;

/// What the agent's pull request tools go through: the strip's own service.
pub trait AgentPullRequests: Send + Sync {
    /// `pullRequestStatus`: the strip's read, for the folder's branch or
    /// another one, leaving the strip's state alone. A server out of reach is
    /// an `Err` here — the model is told, not shown nothing.
    fn status(&self, root: &std::path::Path, branch: Option<&str>) -> Result<PrStrip, CodeHostError>;
    /// `pullRequestCreate`: the strip's Create — from the checked-out branch,
    /// into `to` or the default branch, refused while the branch is not
    /// pushed; the strip reads again after it.
    fn create(&self, root: &std::path::Path, title: &str, description: &str, to: Option<&str>, draft: bool)
        -> Result<PullRequest, CodeHostError>;
}

/// Whether the token's user has approved this pull request.
pub fn approved_by(pr: &PullRequest, me: &str) -> bool {
    let me = me.trim();
    !me.is_empty()
        && pr
            .reviewers
            .iter()
            .any(|reviewer| reviewer.slug.trim().eq_ignore_ascii_case(me) && reviewer.status == ReviewStatus::Approved)
}

/// Whether the strip offers Approve: never on one's own pull request — the
/// server refuses it — and not again once this reviewer has approved.
pub fn can_approve(pr: &PullRequest, me: &str) -> bool {
    let me = me.trim();
    // Who the reviewer is has not been read yet: no button, rather than one the
    // server refuses.
    if me.is_empty() {
        return false;
    }
    let is = |slug: &str| slug.trim().eq_ignore_ascii_case(me);
    !is(&pr.author)
        && pr.reviewers.iter().find(|reviewer| is(&reviewer.slug)).map(|reviewer| reviewer.status)
            != Some(ReviewStatus::Approved)
}

/// What stands between the pull request and Merge, as the strip lists it under
/// the button — the server's veto lines, and a conflict when there is one.
/// `None`: nothing does, and Merge is offered.
pub fn merge_blocked(merge: &MergeCheck) -> Option<Vec<String>> {
    if merge.can_merge && !merge.conflicted {
        return None;
    }

    let mut reasons = Vec::new();
    if merge.conflicted {
        reasons.push("conflicts".to_string());
    }
    reasons.extend(merge.vetoes.iter().cloned());
    // Not mergeable with nothing said would leave a disabled button and no
    // reason under it, which is the one thing decision 6 forbids.
    if reasons.is_empty() {
        reasons.push("the server does not allow a merge".to_string());
    }
    Some(reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str, base_url: &str, hosts: &[&str]) -> CodeHostServer {
        CodeHostServer {
            id: id.to_string(),
            base_url: base_url.to_string(),
            hosts: hosts.iter().map(|host| host.to_string()).collect(),
            trusted_cert_pem: None,
            token_set: true,
        }
    }

    fn pr(author: &str, reviewers: &[(&str, ReviewStatus)]) -> PullRequest {
        PullRequest {
            id: 153,
            title: "a pull request".to_string(),
            version: 6,
            author: author.to_string(),
            author_name: author.to_string(),
            draft: Some(false),
            from_branch: "docs/wowtax-8238".to_string(),
            to_branch: "master".to_string(),
            reviewers: reviewers
                .iter()
                .map(|(slug, status)| Reviewer { slug: slug.to_string(), display_name: slug.to_string(), status: *status })
                .collect(),
            url: "https://server.com/projects/P/repos/r/pull-requests/153".to_string(),
        }
    }

    // --------------------------------------------------------------- remotes

    /// The probe's own answers: one repository, four ways of writing it.
    #[test]
    fn the_ways_of_writing_one_remote_are_one_repository() {
        let ssh = RemoteRepo::parse(
            "ssh://git@server.com/corp-wowtax/corp-wowtax-tax-transaction-api.git",
        )
        .unwrap();
        assert_eq!(ssh.canonical(), "server.com/corp-wowtax/corp-wowtax-tax-transaction-api");

        for other in [
            "ssh://git@server.com:7999/corp-wowtax/corp-wowtax-tax-transaction-api.git",
            "git@server.com:corp-wowtax/corp-wowtax-tax-transaction-api.git",
            "https://server.com/scm/corp-wowtax/corp-wowtax-tax-transaction-api.git",
            "https://USER:TOKEN@server.com/scm/corp-wowtax/corp-wowtax-tax-transaction-api",
            "  https://server.com/scm/corp-wowtax/corp-wowtax-tax-transaction-api/  ",
            "ssh://git@SERVER.COM/corp-wowtax/corp-wowtax-tax-transaction-api.git",
        ] {
            assert_eq!(RemoteRepo::parse(other).unwrap().canonical(), ssh.canonical(), "{other}");
        }
    }

    /// A project key is case-insensitive, and the web spells it upper where the
    /// API uses lower. Lowercasing the slug too would merge two repositories on
    /// a host that does have case-sensitive paths.
    #[test]
    fn the_project_key_is_lowercased_and_the_slug_is_not() {
        let upper = RemoteRepo::parse("https://server.com/scm/CORP-WOWTAX/Repo.git").unwrap();
        assert_eq!(upper.project, "CORP-WOWTAX");
        assert_eq!(upper.canonical(), "server.com/corp-wowtax/Repo");
    }

    /// `.git` is not part of a name: one repository, however the suffix is typed.
    #[test]
    fn a_dot_git_in_any_case_is_not_part_of_the_name() {
        let plain = RemoteRepo::parse("https://server.com/scm/P/repo").unwrap();
        for other in ["https://server.com/scm/P/repo.git", "https://server.com/scm/P/repo.GIT"] {
            assert_eq!(RemoteRepo::parse(other).unwrap().canonical(), plain.canonical(), "{other}");
        }
    }

    /// A token in a clone URL is ordinary — CI writes them — and an error reaches
    /// the window and the agent's call log, which may show neither.
    #[test]
    fn an_error_never_repeats_a_credential_from_the_address() {
        let error = RemoteRepo::parse("https://me:ATATT-secret@server.com/org/repo").unwrap_err();
        let text = error.to_string();
        assert!(!text.contains("ATATT"), "{text}");
        assert!(text.contains("server.com/org/repo"), "{text}");
    }

    /// A password may carry an `@` of its own: the last one ends the userinfo,
    /// and nothing of the password stays in front of the host.
    #[test]
    fn a_password_with_an_at_sign_is_dropped_whole() {
        let parsed = RemoteRepo::parse("https://me:p@ss-ATATT@server.com/scm/P/r.git").unwrap();
        assert_eq!(parsed.host, "server.com");
        assert_eq!(parsed.canonical(), "server.com/p/r");

        let text = RemoteRepo::parse("https://me:p@ss-ATATT@server.com/org/repo").unwrap_err().to_string();
        assert!(!text.contains("ATATT") && !text.contains("ss-"), "{text}");
        assert!(text.contains("https://server.com/org/repo"), "{text}");
    }

    /// An `@` after the host is part of the path, not the end of a userinfo.
    #[test]
    fn an_at_sign_in_the_path_is_left_where_it_is() {
        assert_eq!(host_of("https://code.corp/x@y"), Some("code.corp".to_string()));
        assert_eq!(without_credentials("https://code.corp/a@b/c"), "https://code.corp/a@b/c");
        assert_eq!(without_credentials("git@code.corp:P/r.git"), "code.corp:P/r.git");
    }

    /// A context path is part of the web address the form offers, and no part of
    /// what decides which repository this is.
    #[test]
    fn the_context_path_reaches_the_base_address_only() {
        let behind_context = RemoteRepo::parse("https://server.com/bitbucket/scm/P/r.git").unwrap();
        assert_eq!(behind_context.base_url, "https://server.com/bitbucket");
        assert_eq!(behind_context.canonical(), "server.com/p/r");
    }

    /// The probe's server answers ssh on 22 and the web on 443: the ssh port is
    /// the ssh port, and an ssh clone says nothing about the web address.
    #[test]
    fn an_ssh_remote_offers_the_host_and_no_port() {
        for remote in ["ssh://git@server.com:7999/P/r.git", "git@server.com:P/r.git"] {
            let parsed = RemoteRepo::parse(remote).unwrap();
            assert_eq!(parsed.base_url, "https://server.com", "{remote}");
            assert_eq!(parsed.transport, Transport::Ssh, "{remote}");
        }
    }

    #[test]
    fn a_web_clone_keeps_its_port_and_the_form_proposes_https() {
        let parsed = RemoteRepo::parse("http://server.com:8443/scm/P/r.git").unwrap();
        assert_eq!(parsed.base_url, "https://server.com:8443");
        assert_eq!(parsed.host, "server.com");
        assert_eq!(parsed.transport, Transport::Web);
    }

    /// Data Center puts a personal repository where the project key goes.
    #[test]
    fn a_personal_repository_keeps_its_owner_where_the_project_would_be() {
        for remote in ["ssh://git@server.com:7999/~user/repo.git", "https://server.com/scm/~user/repo"] {
            let parsed = RemoteRepo::parse(remote).unwrap();
            assert_eq!(parsed.project, "~user", "{remote}");
            assert_eq!(parsed.slug, "repo", "{remote}");
        }
    }

    /// The guess stops where the address stops being one: a form gets the base
    /// address and empty fields, never a project made out of a host name.
    #[test]
    fn an_address_without_a_project_and_a_repository_is_refused() {
        for remote in [
            "",
            "https://server.com",
            "https://server.com/",
            "https://server.com/org/repo",
            "https://server.com/scm/P",
            "https://server.com/scm/P/r/extra",
            "git@server.com:repo.git",
            "/Users/me/repo",
            "https://server.com:notaport/scm/P/r.git",
            "file:///tmp/P/r.git",
        ] {
            assert!(RemoteRepo::parse(remote).is_err(), "{remote} parsed");
        }
    }

    // -------------------------------------------------------------- servers

    #[test]
    fn a_server_is_found_by_its_own_host_or_by_a_remembered_alias() {
        let own = server("a", "https://server.com", &[]);
        let alias = server("b", "https://other.example", &["git.server.com"]);
        let servers = [own.clone(), alias.clone()];

        let by_host = RemoteRepo::parse("ssh://git@server.com:7999/P/r.git").unwrap();
        assert_eq!(server_for(&servers, &by_host).map(|s| s.id.clone()), Some(own.id));

        let by_alias = RemoteRepo::parse("ssh://git@git.server.com/P/r.git").unwrap();
        assert_eq!(server_for(&servers, &by_alias).map(|s| s.id.clone()), Some(alias.id));
    }

    /// A host is case-insensitive wherever it is written — in the remote the
    /// folder carries and in the address the user typed into the form.
    #[test]
    fn a_host_in_any_case_is_the_same_host() {
        let servers = [server("a", "https://SERVER.com", &["Git.Server.com"])];

        let own = RemoteRepo::parse("ssh://git@server.com/P/r.git").unwrap();
        assert_eq!(server_for(&servers, &own).map(|s| s.id.clone()), Some("a".to_string()));

        let alias = RemoteRepo::parse("ssh://git@git.server.com/P/r.git").unwrap();
        assert_eq!(server_for(&servers, &alias).map(|s| s.id.clone()), Some("a".to_string()));
    }

    /// What a server is found by, from whatever was typed into the form.
    #[test]
    fn a_base_address_is_read_down_to_its_host() {
        assert_eq!(host_of("https://code.corp:8443/bitbucket"), Some("code.corp".to_string()));
        assert_eq!(host_of("https://CODE.corp/"), Some("code.corp".to_string()));
        assert_eq!(host_of("https://user:token@code.corp/x"), Some("code.corp".to_string()));
        assert_eq!(host_of("code.corp/x"), Some("code.corp".to_string()));
        assert_eq!(host_of("http://[::1]:8080/x"), Some("::1".to_string()));
        assert_eq!(host_of("http://localhost?x=1"), Some("localhost".to_string()));
        assert_eq!(host_of("  "), None);
        assert_eq!(host_of("https://"), None);
    }

    /// Two servers can live on one host and differ by context path or port, and
    /// only the address the remote implies tells them apart.
    #[test]
    fn two_servers_on_one_host_are_told_apart_by_the_remote_s_own_address() {
        let in_context = server("bb", "https://code.corp/bitbucket", &[]);
        let at_root = server("root", "https://code.corp", &[]);
        let servers = [in_context.clone(), at_root.clone()];

        let context_remote = RemoteRepo::parse("https://code.corp/bitbucket/scm/P/r.git").unwrap();
        assert_eq!(server_for(&servers, &context_remote).map(|s| s.id.clone()), Some(in_context.id));

        let root_remote = RemoteRepo::parse("https://code.corp/scm/P/r.git").unwrap();
        assert_eq!(server_for(&servers, &root_remote).map(|s| s.id.clone()), Some(at_root.id));
    }

    /// An ssh remote implies `https://host` and nothing more, so two instances on
    /// one host stay unanswered: the user picks, not the first entry.
    #[test]
    fn an_ssh_remote_that_two_servers_answer_to_picks_neither() {
        let servers = [server("one", "https://code.corp:8443", &[]), server("two", "https://code.corp:9443", &[])];
        let ssh = RemoteRepo::parse("ssh://git@code.corp/P/r.git").unwrap();
        assert!(server_for(&servers, &ssh).is_none());
    }

    /// The one rule a token depends on: a host nobody added gets no server, not
    /// the first one configured.
    #[test]
    fn a_remote_on_no_configured_server_picks_nothing() {
        let servers = [server("a", "https://server.com", &["git.server.com"])];
        for remote in ["ssh://git@elsewhere.example/P/r.git", "https://github.com/scm/P/r.git"] {
            let parsed = RemoteRepo::parse(remote).unwrap();
            assert!(server_for(&servers, &parsed).is_none(), "{remote}");
        }
    }

    #[test]
    fn a_host_the_server_already_answers_to_is_not_remembered_again() {
        let servers = server("a", "https://server.com", &["git.server.com"]);

        let own = RemoteRepo::parse("https://server.com/scm/P/r.git").unwrap();
        assert_eq!(alias_to_remember(&servers, &own), None);

        let alias = RemoteRepo::parse("ssh://git@GIT.SERVER.com/P/r.git").unwrap();
        assert_eq!(alias_to_remember(&servers, &alias), None);

        let unknown = RemoteRepo::parse("ssh://git@bitbucket.example/P/r.git").unwrap();
        assert_eq!(alias_to_remember(&servers, &unknown), Some("bitbucket.example".to_string()));
    }

    // -------------------------------------------------------------- buttons

    #[test]
    fn the_author_cannot_approve_their_own_pull_request() {
        let mine = pr("u_me", &[("u_me", ReviewStatus::Unapproved)]);
        assert!(!can_approve(&mine, "u_me"));

        let theirs = pr("u_them", &[("u_me", ReviewStatus::Unapproved)]);
        assert!(can_approve(&theirs, "u_me"));
    }

    /// Who the reviewer is may not be known yet — a proxy can cut the header off
    /// — and then no button beats one the server refuses.
    #[test]
    fn an_unknown_reviewer_gets_no_approve() {
        let theirs = pr("u_them", &[]);
        assert!(!can_approve(&theirs, ""));
        assert!(!can_approve(&theirs, "   "));
    }

    /// A slug is the same slug in any case, whichever spelling the header and the
    /// server use.
    #[test]
    fn a_slug_in_other_case_is_the_same_reviewer() {
        assert!(!can_approve(&pr("u_me", &[]), "U_ME"), "our own pull request");
        let approved = pr("u_them", &[("U_ME", ReviewStatus::Approved)]);
        assert!(!can_approve(&approved, "u_me"), "already approved");
    }

    #[test]
    fn an_approved_reviewer_is_offered_no_approve_and_a_needs_work_is() {
        let approved = pr("u_them", &[("u_me", ReviewStatus::Approved)]);
        assert!(!can_approve(&approved, "u_me"));

        let needs_work = pr("u_them", &[("u_me", ReviewStatus::NeedsWork)]);
        assert!(can_approve(&needs_work, "u_me"));

        let not_a_reviewer = pr("u_them", &[("u_other", ReviewStatus::Unapproved)]);
        assert!(can_approve(&not_a_reviewer, "u_me"));
    }

    /// When the server says `canMerge`, a veto line beside it is a leftover and
    /// the button stays live: its word beats its own list.
    #[test]
    fn the_server_s_word_wins_over_a_veto_line_of_its_own() {
        let odd = MergeCheck { can_merge: true, conflicted: false, vetoes: vec!["Requires approvals".to_string()] };
        assert_eq!(merge_blocked(&odd), None);
    }

    #[test]
    fn a_mergeable_pull_request_has_nothing_under_the_button() {
        let clean = MergeCheck { can_merge: true, conflicted: false, vetoes: Vec::new() };
        assert_eq!(merge_blocked(&clean), None);
    }

    /// The probe's own answer: three veto lines, shown as the server wrote them.
    #[test]
    fn the_server_s_words_are_what_stands_under_a_blocked_merge() {
        let blocked = MergeCheck {
            can_merge: false,
            conflicted: false,
            vetoes: vec!["Requires approvals".to_string(), "Not all required builds are successful yet".to_string()],
        };
        assert_eq!(
            merge_blocked(&blocked),
            Some(vec![
                "Requires approvals".to_string(),
                "Not all required builds are successful yet".to_string()
            ])
        );
    }

    #[test]
    fn a_conflict_is_named_and_comes_before_the_vetoes() {
        let conflicted = MergeCheck {
            can_merge: false,
            conflicted: true,
            vetoes: vec!["Requires approvals".to_string()],
        };
        assert_eq!(
            merge_blocked(&conflicted),
            Some(vec!["conflicts".to_string(), "Requires approvals".to_string()])
        );
    }

    /// A conflict is a conflict: a server that says `canMerge` anyway (and the
    /// probe's said `conflicted=false`, so this is the belt beside the braces)
    /// still gets the reason named.
    #[test]
    fn a_conflict_blocks_even_when_the_server_calls_it_mergeable() {
        let odd = MergeCheck { can_merge: true, conflicted: true, vetoes: Vec::new() };
        assert_eq!(merge_blocked(&odd), Some(vec!["conflicts".to_string()]));
    }

    /// A disabled button with nothing under it is exactly what decision 6
    /// forbids, so a silent refusal still gets a line.
    #[test]
    fn a_refusal_with_no_veto_still_says_something() {
        let silent = MergeCheck { can_merge: false, conflicted: false, vetoes: Vec::new() };
        assert_eq!(merge_blocked(&silent), Some(vec!["the server does not allow a merge".to_string()]));
    }

    /// Unapprove is offered for the token's own approval, in any case of its
    /// slug, and for no one else's.
    #[test]
    fn approved_is_the_token_s_user_approving() {
        let mine = pr("u_them", &[("U_ME", ReviewStatus::Approved), ("u_other", ReviewStatus::Unapproved)]);
        assert!(approved_by(&mine, "u_me"));
        assert!(!approved_by(&mine, "u_other"));
        assert!(!approved_by(&mine, "u_nobody"));
        assert!(!approved_by(&mine, " "));
        assert!(!approved_by(&pr("u_them", &[("u_me", ReviewStatus::NeedsWork)]), "u_me"));
    }

    fn build(name: &str, state: BuildState, date_added: Option<u64>) -> Build {
        Build { key: name.to_string(), name: name.to_string(), state, url: String::new(), date_added }
    }

    /// A run started after one that finished is the one to show — and a run
    /// left "in progress" by a dead job, older than a finished one, is not.
    #[test]
    fn the_latest_build_is_the_newest_the_server_took() {
        let latest = |builds: &[Build]| latest_build(builds).map(|build| build.name.clone());
        assert_eq!(latest(&[]), None);
        let done = build("#211", BuildState::Successful, Some(1_000));
        let running = build("#212", BuildState::InProgress, Some(2_000));
        assert_eq!(latest(&[running.clone(), done.clone()]).as_deref(), Some("#212"));
        let stuck = build("#210", BuildState::InProgress, Some(500));
        assert_eq!(latest(&[stuck, done.clone()]).as_deref(), Some("#211"));

        // Undated: older than any dated; among themselves a running one, then the last.
        let undated_done = build("old", BuildState::Successful, None);
        assert_eq!(latest(&[done.clone(), undated_done.clone()]).as_deref(), Some("#211"));
        let undated_running = build("run", BuildState::InProgress, None);
        assert_eq!(latest(&[undated_running, undated_done.clone()]).as_deref(), Some("run"));
        let other = build("other", BuildState::Failed, None);
        assert_eq!(latest(&[undated_done, other]).as_deref(), Some("other"));
    }

    #[test]
    fn a_strip_is_building_while_its_latest_build_runs() {
        let ready = |latest: Option<BuildState>| PrStrip::Ready {
            server_id: "s".to_string(),
            repo: "r".to_string(),
            branch: "b".to_string(),
            snapshot: Snapshot::default(),
            can_approve: false,
            approved: false,
            merge_blocked: None,
            latest_build: latest.map(|state| build("n", state, None)),
        };
        assert!(ready(Some(BuildState::InProgress)).building());
        assert!(!ready(Some(BuildState::Successful)).building());
        assert!(!ready(None).building());
        assert!(!PrStrip::NoRemote.building());
    }

    /// The window's `PrStrip` union in `src/lib/pullRequests.ts` reads these
    /// names: a tag of `state`, camelCase inside each.
    #[test]
    fn a_strip_reaches_the_window_in_the_shape_it_reads() {
        let json = |strip: &PrStrip| serde_json::to_value(strip).unwrap();
        assert_eq!(json(&PrStrip::NoRemote), serde_json::json!({ "state": "noRemote" }));
        assert_eq!(json(&PrStrip::NoBranch), serde_json::json!({ "state": "noBranch" }));
        assert_eq!(json(&PrStrip::Unreachable), serde_json::json!({ "state": "unreachable" }));

        let remote = RemoteRepo::parse("ssh://git@git.server.com/corp/tax-api.git").unwrap();
        let not_connected = json(&PrStrip::NotConnected { remote });
        assert_eq!((&not_connected["state"], &not_connected["remote"]["baseUrl"]), (&"notConnected".into(), &"https://git.server.com".into()));

        let ready = json(&PrStrip::Ready {
            server_id: "s".to_string(),
            repo: "r".to_string(),
            branch: "b".to_string(),
            snapshot: snapshot(),
            can_approve: true,
            approved: false,
            merge_blocked: Some(vec!["v".to_string()]),
            latest_build: snapshot().builds.into_iter().next(),
        });
        for key in ["state", "serverId", "repo", "branch", "snapshot", "canApprove", "approved", "mergeBlocked", "latestBuild"] {
            assert!(ready.get(key).is_some(), "{key}: {ready}");
        }
        assert_eq!(ready["state"], "ready");
        assert!(ready["snapshot"].get("branchOnServer").is_some(), "{ready}");
        assert!(ready["latestBuild"].get("dateAdded").is_some(), "{ready}");
        assert_eq!(ready["snapshot"]["pr"]["draft"], false, "{ready}");
        let older = serde_json::to_value(PullRequest { draft: None, ..pr("u", &[]) }).unwrap();
        assert_eq!(older["draft"], serde_json::Value::Null, "a server before drafts says neither: {older}");
    }

    // ------------------------------------------------------------- snapshots

    fn snapshot() -> Snapshot {
        Snapshot {
            pr: Some(pr("u_them", &[("u_me", ReviewStatus::Unapproved)])),
            branch_on_server: true,
            default_branch: Some("master".to_string()),
            merge: Some(MergeCheck { can_merge: false, conflicted: false, vetoes: vec!["Requires approvals".to_string()] }),
            builds: vec![Build {
                key: "CI".to_string(),
                name: "pipeline".to_string(),
                state: BuildState::InProgress,
                url: "https://server.com/builds/1".to_string(),
                date_added: Some(1_700_000_000_000),
            }],
        }
    }

    /// The watcher's whole contract: an unchanged poll must say nothing, or the
    /// window re-reads every thirty seconds for no reason.
    #[test]
    fn two_snapshots_of_one_state_are_not_a_change() {
        assert_eq!(snapshot(), snapshot());
    }

    #[test]
    fn a_reviewer_a_build_or_a_veto_that_moves_is_a_change() {
        let base = snapshot();

        let mut approved = snapshot();
        approved.pr = Some(pr("u_them", &[("u_me", ReviewStatus::Approved)]));
        assert_ne!(base, approved, "a reviewer approved");

        let mut built = snapshot();
        built.builds[0].state = BuildState::Successful;
        assert_ne!(base, built, "a build finished");

        let mut unblocked = snapshot();
        unblocked.merge = Some(MergeCheck { can_merge: true, conflicted: false, vetoes: Vec::new() });
        assert_ne!(base, unblocked, "the vetoes went away");
    }

    /// Pushing the branch is a change too: Create PR appears where it was
    /// inactive.
    #[test]
    fn a_branch_that_reaches_the_server_and_a_new_pr_are_changes() {
        let empty = Snapshot::default();
        let mut pushed = Snapshot::default();
        pushed.branch_on_server = true;
        assert_ne!(empty, pushed);

        let mut opened = pushed.clone();
        opened.pr = Some(pr("u_them", &[]));
        assert_ne!(pushed, opened);
    }
}
