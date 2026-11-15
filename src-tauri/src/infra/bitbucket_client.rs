//! Bitbucket Data Center's REST 1.0, as the `CodeHost` a strip asks
//! (`docs/56-bitbucket-prs.md`, decisions 2, 3 and 9).
//!
//! Every address and every shape read here was answered by the probe of that
//! plan against a live 8.19.26; the tests' JSON is those answers with the names
//! taken out. Two of them settled decisions: the builds endpoint under the
//! repository answers 400 because it wants a `key` it is never given, so the
//! legacy `/rest/build-status` path is the one read; and the merge check hands
//! back its veto lines ready to show.
//!
//! Nothing here keeps a token: [`Call`] carries it per request, and it goes in
//! one header, to the address the user added and nowhere else.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::code_host::{
    host_of, Build, BuildState, Call, CodeHost, CodeHostError, CodeHostServer, MergeCheck, PullRequest, ReviewStatus,
    Reviewer,
};
use crate::infra::http_agent;

/// A read answers in about a second on the probed server; a minute of nothing
/// is a server that is not coming back this poll.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Of an error body, what the strip shows.
const ERROR_CHARS: usize = 200;

/// Data Center's API, in its own names. The one place Bitbucket is spoken.
pub struct Bitbucket;

impl CodeHost for Bitbucket {
    fn identify(&self, server: &CodeHostServer) -> Result<String, CodeHostError> {
        // No token: the form asks this before one is saved, and the probe saw
        // the server answer it to anyone.
        let url = format!("{}/rest/api/latest/application-properties", server.base_url.trim().trim_end_matches('/'));
        let (_, text) = answer(server, common(http::Method::GET, &url), None)?;
        let properties: Properties = read(&text)?;
        if properties.display_name != "Bitbucket" {
            return Err(CodeHostError::BadAnswer(format!(
                "the server calls itself \"{}\", not Bitbucket",
                properties.display_name
            )));
        }
        Ok(format!("Bitbucket {}", properties.version).trim().to_string())
    }

    fn clone_links(&self, call: &Call) -> Result<Vec<String>, CodeHostError> {
        let repo: Repo = read(&get(call, &api(call))?)?;
        Ok(repo.links.clone_links.into_iter().map(|link| link.href).collect())
    }

    fn me(&self, call: &Call) -> Result<String, CodeHostError> {
        let (headers, _) = request(call, http::Method::GET, &api(call), None)?;
        headers
            .get("x-ausername")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                CodeHostError::BadAnswer(
                    "the server did not say whose token this is (no X-AUSERNAME header)".to_string(),
                )
            })
    }

    fn default_branch(&self, call: &Call) -> Result<String, CodeHostError> {
        let branch: Branch = read(&get(call, &format!("{}/default-branch", api(call)))?)?;
        Ok(branch.display_id)
    }

    fn branch_commit(&self, call: &Call, branch: &str) -> Result<Option<String>, CodeHostError> {
        let found: Values<BranchCommit> =
            read(&get(call, &format!("{}/branches?filterText={}&boostMatches=true&limit=25", api(call), encode(branch)))?)?;
        // `filterText` is a substring search: the server's list is searched for
        // the branch itself, not taken as the answer. `boostMatches` puts the
        // exact name first, or `dev` among thirty `feature/dev-*` would fall off
        // the page and read as a branch nobody pushed.
        Ok(found.values.into_iter().find(|candidate| candidate.display_id == branch).and_then(|candidate| candidate.latest_commit))
    }

    fn pull_request(&self, call: &Call, branch: &str) -> Result<Option<PullRequest>, CodeHostError> {
        let at = encode(&format!("refs/heads/{branch}"));
        let found: Values<Pr> = read(&get(
            call,
            &format!("{}/pull-requests?at={at}&direction=OUTGOING&state=OPEN", api(call)),
        )?)?;
        Ok(found.values.into_iter().next().map(Pr::into_pull_request))
    }

    fn merge_check(&self, call: &Call, id: u64) -> Result<MergeCheck, CodeHostError> {
        let merge: Merge = read(&get(call, &format!("{}/pull-requests/{id}/merge", api(call)))?)?;
        Ok(MergeCheck {
            can_merge: merge.can_merge.unwrap_or(false),
            conflicted: merge.conflicted.unwrap_or(false),
            vetoes: merge
                .vetoes
                .into_iter()
                .map(|veto| veto.summary_message)
                .filter(|line| !line.trim().is_empty())
                .collect(),
        })
    }

    fn builds(&self, call: &Call, commit: &str) -> Result<Vec<Build>, CodeHostError> {
        // The probe's finding: `/repos/{r}/commits/{sha}/builds` answers 400
        // ("specify a non-blank key") — it wants one build's key, not the list —
        // so the legacy path is the one that lists them.
        let found: Values<BuildJson> =
            read(&get(call, &format!("{}/rest/build-status/latest/commits/{}", base(call), encode(commit)))?)?;
        Ok(found
            .values
            .into_iter()
            .map(|build| Build {
                key: build.key,
                name: build.name,
                state: build_state(&build.state),
                url: build.url,
            })
            .collect())
    }

    fn approve(&self, call: &Call, pr: &PullRequest, me: &str, approved: bool) -> Result<(), CodeHostError> {
        let status = if approved { "APPROVED" } else { "UNAPPROVED" };
        put(
            call,
            &format!("{}/pull-requests/{}/participants/{}", api(call), pr.id, encode(me)),
            json!({ "status": status }),
        )
        .map(|_| ())
    }

    fn merge(&self, call: &Call, pr: &PullRequest) -> Result<(), CodeHostError> {
        // The version is the optimistic lock: a stale one is 409, and the strip
        // re-reads rather than repeating this. The empty object is there for the
        // `Content-Type`: the resource takes JSON, and a body-less POST is 415.
        post(call, &format!("{}/pull-requests/{}/merge?version={}", api(call), pr.id, pr.version), json!({})).map(|_| ())
    }

    fn create(&self, call: &Call, from: &str, to: &str, title: &str) -> Result<PullRequest, CodeHostError> {
        let body = json!({
            "title": title,
            "fromRef": { "id": format!("refs/heads/{from}") },
            "toRef": { "id": format!("refs/heads/{to}") },
        });
        let created = post(call, &format!("{}/pull-requests", api(call)), body)?;
        Ok(read::<Pr>(&created)?.into_pull_request())
    }
}

// ------------------------------------------------------------------ requests

fn base<'a>(call: &'a Call) -> &'a str {
    call.server.base_url.trim_end_matches('/')
}

fn api(call: &Call) -> String {
    format!("{}/rest/api/latest/projects/{}/repos/{}", base(call), encode(&call.remote.project), encode(&call.remote.slug))
}

fn bearer(call: &Call) -> String {
    format!("Bearer {}", call.token)
}

/// The name the app gives itself in a request — the same header the providers'
/// agent sends, so a WAF that refuses an unnamed client at least says which.
const AGENT: &str = concat!("Kibo-Agent/", env!("CARGO_PKG_VERSION"));

/// One request, with what every one of them shares: the token in one header, a
/// timeout, and no redirect followed. A body is JSON; a request without one is
/// a read.
///
/// The redirect is not a nicety: Data Center answers 302 to an SSO page (the
/// probe's step 3 looks for exactly that), and following it would carry the
/// `Authorization` header to whichever host the answer names — the one thing
/// decision 1 promises never happens.
///
/// `X-Atlassian-Token: no-check` is what Atlassian's XSRF filter wants of a
/// client that is not a browser; without it a write can come back 403, which
/// would read as a token without the right to write.
fn request(
    call: &Call,
    method: http::Method,
    url: &str,
    body: Option<Value>,
) -> Result<(http::HeaderMap, String), CodeHostError> {
    answer(call.server, common(method, url).header("Authorization", bearer(call)), body)
}

/// The headers every request carries, the token aside.
fn common(method: http::Method, url: &str) -> http::request::Builder {
    http::Request::builder()
        .method(method)
        .uri(url)
        .header("Accept", "application/json")
        .header("User-Agent", AGENT)
        .header("X-Atlassian-Token", "no-check")
}

/// Sends `request` to `server` and reads the answer through [`status_check`].
fn answer(
    server: &CodeHostServer,
    request: http::request::Builder,
    body: Option<Value>,
) -> Result<(http::HeaderMap, String), CodeHostError> {
    // A token goes over TLS or nowhere; `http` is left for this machine alone,
    // where a test's own server listens — the rule the catalog's sources use.
    let base = server.base_url.trim().trim_end_matches('/');
    if base.starts_with("http://") && !is_loopback(base) {
        return Err(CodeHostError::Insecure(base.to_string()));
    }
    // The server's own certificate replaces the public roots: a corporate CA
    // is trusted for this server and nowhere else.
    let agent = http_agent::build_agent(server.trusted_cert_pem.as_deref()).map_err(|e| CodeHostError::Tls(e.0))?;
    let mut response = match body {
        None => send(&agent, request.body(())),
        Some(body) => send(&agent, request.header("Content-Type", "application/json").body(body.to_string())),
    }?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| CodeHostError::Network(e.to_string()))?;
    status_check(status, &text)?;
    Ok((std::mem::take(response.headers_mut()), text))
}

fn send<S: ureq::AsSendBody>(
    agent: &ureq::Agent,
    request: Result<http::Request<S>, http::Error>,
) -> Result<http::Response<ureq::Body>, CodeHostError> {
    // A header the token makes invalid — a newline pasted with it — fails here,
    // before anything is sent.
    let request = request.map_err(|e| CodeHostError::Network(e.to_string()))?;
    let request = agent.configure_request(request).timeout_global(Some(TIMEOUT)).max_redirects(0).build();
    agent.run(request).map_err(|error| match error {
        ureq::Error::Tls(message) => CodeHostError::Tls(message.to_string()),
        // A rustls handshake failure reaches ureq as an io error, and its text is
        // rustls' own — "invalid peer certificate: UnknownIssuer". That text is
        // the only thing telling a refused certificate from a closed port, and a
        // server behind an untrusted CA is exactly what this names.
        ureq::Error::Io(e) if e.to_string().contains("certificate") => CodeHostError::Tls(e.to_string()),
        other => CodeHostError::Network(other.to_string()),
    })
}

/// `http` is allowed only to this machine, where a test's own server listens.
/// The host is compared whole: `localhost.example` and `localhost@example` are
/// somewhere else.
fn is_loopback(base: &str) -> bool {
    match host_of(base) {
        Some(host) => host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn get(call: &Call, url: &str) -> Result<String, CodeHostError> {
    request(call, http::Method::GET, url, None).map(|(_, text)| text)
}

fn post(call: &Call, url: &str, body: Value) -> Result<String, CodeHostError> {
    request(call, http::Method::POST, url, Some(body)).map(|(_, text)| text)
}

fn put(call: &Call, url: &str, body: Value) -> Result<String, CodeHostError> {
    request(call, http::Method::PUT, url, Some(body)).map(|(_, text)| text)
}

/// The three statuses the strip acts on, and everything else with the server's
/// own words: 401 is the token, 403 the token's rights, 409 a stale version.
///
/// Only a stale version is [`CodeHostError::Conflict`]: Data Center answers 409
/// to a vetoed merge and to a second pull request between the same branches as
/// well, and there its own words are the whole point.
fn status_check(status: u16, text: &str) -> Result<(), CodeHostError> {
    match status {
        200..=299 => Ok(()),
        401 => Err(CodeHostError::Unauthorized),
        403 => Err(CodeHostError::Forbidden),
        409 if is_out_of_date(text) => Err(CodeHostError::Conflict),
        _ => Err(CodeHostError::Http(status, said(text))),
    }
}

/// Whether a 409 is the optimistic lock: Data Center names the exception
/// `…PullRequestOutOfDateException`.
fn is_out_of_date(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|value| {
        value["errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|error| error["exceptionName"].as_str().is_some_and(|name| name.ends_with("OutOfDateException"))))
    })
}

fn read<T: DeserializeOwned>(text: &str) -> Result<T, CodeHostError> {
    serde_json::from_str(text).map_err(|e| CodeHostError::BadAnswer(e.to_string()))
}

/// A path or query segment as the API takes it: a branch's `/` becomes `%2F`,
/// which is what the probe sent for `at=refs%2Fheads%2Fdocs%2Fwowtax-8238`.
fn encode(segment: &str) -> String {
    // RFC 3986's unreserved set, kept; everything else percent-encoded.
    const KEPT: &percent_encoding::AsciiSet =
        &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
    percent_encoding::utf8_percent_encode(segment, KEPT).to_string()
}

/// The server's own words: its `errors[].message`, or the body, cut short.
fn said(text: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| value["errors"][0]["message"].as_str().or_else(|| value["message"].as_str()));
    let text = message.unwrap_or(text).trim();
    match text.char_indices().nth(ERROR_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// `APPROVED` is the only yes; a `NEEDS_WORK` is a decision too; anything this
/// app has no word for counts as not approved — the strip shows it as a
/// reviewer who has not said yes.
fn review_status(state: &str) -> ReviewStatus {
    match state {
        "APPROVED" => ReviewStatus::Approved,
        "NEEDS_WORK" => ReviewStatus::NeedsWork,
        _ => ReviewStatus::Unapproved,
    }
}

/// A state this app has no word for is never a success.
fn build_state(state: &str) -> BuildState {
    match state {
        "SUCCESSFUL" => BuildState::Successful,
        "FAILED" => BuildState::Failed,
        "INPROGRESS" => BuildState::InProgress,
        _ => BuildState::Unknown,
    }
}

// ----------------------------------------------------------- the server's JSON

#[derive(Deserialize)]
struct Properties {
    #[serde(default, rename = "displayName")]
    display_name: String,
    #[serde(default)]
    version: String,
}

#[derive(Deserialize)]
struct Values<T> {
    #[serde(default = "Vec::new")]
    values: Vec<T>,
}

#[derive(Deserialize)]
struct Repo {
    #[serde(default)]
    links: RepoLinks,
}

#[derive(Deserialize, Default)]
struct RepoLinks {
    #[serde(default, rename = "clone")]
    clone_links: Vec<Link>,
}

#[derive(Deserialize)]
struct Link {
    href: String,
}

#[derive(Deserialize)]
struct Branch {
    #[serde(rename = "displayId")]
    display_id: String,
}

#[derive(Deserialize)]
struct BranchCommit {
    #[serde(rename = "displayId")]
    display_id: String,
    #[serde(default, rename = "latestCommit")]
    latest_commit: Option<String>,
}

#[derive(Deserialize)]
struct Pr {
    id: u64,
    #[serde(default)]
    version: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: Person,
    #[serde(default, rename = "fromRef")]
    from_ref: GitRef,
    #[serde(default, rename = "toRef")]
    to_ref: GitRef,
    #[serde(default)]
    reviewers: Vec<PrReviewer>,
    #[serde(default)]
    links: PrLinks,
}

impl Pr {
    fn into_pull_request(self) -> PullRequest {
        PullRequest {
            id: self.id,
            title: self.title,
            version: self.version,
            author: self.author.user.slug,
            from_branch: self.from_ref.display_id,
            to_branch: self.to_ref.display_id,
            reviewers: self
                .reviewers
                .into_iter()
                .filter(|reviewer| !reviewer.user.slug.is_empty())
                .map(|reviewer| Reviewer { slug: reviewer.user.slug, status: review_status(&reviewer.status) })
                .collect(),
            // The probe's answer has only `self`, and it is the page Open
            // opens; `web` is preferred first because a server that names one
            // separately means it.
            url: self
                .links
                .web
                .into_iter()
                .chain(self.links.self_links)
                .next()
                .map(|link| link.href)
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize, Default)]
struct Person {
    #[serde(default)]
    user: User,
}

#[derive(Deserialize, Default)]
struct User {
    #[serde(default)]
    slug: String,
}

#[derive(Deserialize, Default)]
struct GitRef {
    #[serde(default, rename = "displayId")]
    display_id: String,
}

#[derive(Deserialize)]
struct PrReviewer {
    #[serde(default)]
    user: User,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize, Default)]
struct PrLinks {
    #[serde(default)]
    web: Vec<Link>,
    /// The PR's own page, as the probe read it: `…/projects/{KEY}/repos/…/pull-requests/{id}`.
    #[serde(default, rename = "self")]
    self_links: Vec<Link>,
}

#[derive(Deserialize)]
struct Merge {
    #[serde(default, rename = "canMerge")]
    can_merge: Option<bool>,
    #[serde(default)]
    conflicted: Option<bool>,
    #[serde(default)]
    vetoes: Vec<Veto>,
}

#[derive(Deserialize)]
struct Veto {
    #[serde(rename = "summaryMessage")]
    summary_message: String,
}

#[derive(Deserialize)]
struct BuildJson {
    #[serde(default)]
    key: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::code_host::{Call, CodeHostServer, RemoteRepo};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // What the probe of `docs/56-bitbucket-prs.md` saved for this repository,
    // with slugs and titles replaced. The builds list was empty on the probed
    // commit, so its entries are shaped by the field names the legacy path
    // documents rather than copied from an answer.
    const REPO: &str = r#"{"slug":"tax-api","id":1,"name":"tax-api","project":{"key":"CORP","id":2},
        "links":{"clone":[{"href":"ssh://git@server.com/corp/tax-api.git","name":"ssh"},
                          {"href":"https://server.com/scm/corp/tax-api.git","name":"http"}],
                 "self":[{"href":"https://server.com/projects/CORP/repos/tax-api/browse"}]}}"#;

    const PRS: &str = r#"{"size":1,"limit":25,"isLastPage":true,"values":[{"id":153,"version":6,
        "title":"feat(ABC-1): a fix","state":"OPEN","open":true,"closed":false,"draft":false,
        "fromRef":{"id":"refs/heads/docs/wowtax-8238","displayId":"docs/wowtax-8238",
                   "latestCommit":"94beed93512849804d03fe90d10a99b1418d4342","type":"BRANCH"},
        "toRef":{"id":"refs/heads/master","displayId":"master","type":"BRANCH"},
        "author":{"user":{"name":"A","emailAddress":"a@example.com","id":1,"displayName":"A","active":true,"slug":"u_author","type":"NORMAL"}},
        "reviewers":[{"user":{"name":"R","emailAddress":"r@example.com","id":2,"displayName":"R","active":true,"slug":"u_reviewer","type":"NORMAL"},
                      "lastReviewedCommit":"94beed9","approved":false,"status":"UNAPPROVED"}],
        "links":{"self":[{"href":"https://server.com/projects/CORP/repos/tax-api/pull-requests/153"}]}}]}"#;

    const MERGE: &str = r#"{"canMerge":false,"conflicted":false,"outcome":"CLEAN","vetoes":[
        {"summaryMessage":"Include Code Quality detected missing project data","detailedMessage":"No data found."},
        {"summaryMessage":"Not all required builds are successful yet","detailedMessage":"You still need a build."},
        {"summaryMessage":"Requires approvals","detailedMessage":"You still need 1 approval."}]}"#;

    const BUILDS_EMPTY: &str = r#"{"size":0,"limit":25,"isLastPage":true,"values":[],"start":0}"#;

    const BUILDS: &str = r#"{"size":4,"limit":25,"isLastPage":true,"values":[
        {"key":"ci","state":"SUCCESSFUL","name":"build #12","url":"https://ci.example/12","description":""},
        {"key":"cq","state":"FAILED","name":"code quality","url":"https://ci.example/cq"},
        {"key":"scan","state":"INPROGRESS","name":"scan","url":""},
        {"key":"odd","state":"QUEUED","name":"queued","url":""}],"start":0}"#;

    const BRANCHES: &str = r#"{"size":1,"limit":25,"isLastPage":true,"values":[{"id":"refs/heads/docs/wowtax-8238",
        "displayId":"docs/wowtax-8238","type":"BRANCH","latestCommit":"94beed93512849804d03fe90d10a99b1418d4342",
        "latestChangeset":"94beed93512849804d03fe90d10a99b1418d4342","isDefault":false}],"start":0}"#;

    const DEFAULT_BRANCH: &str = r#"{"id":"refs/heads/master","displayId":"master","type":"BRANCH"}"#;

    const CREATED: &str = r#"{"id":154,"version":0,"title":"a title",
        "fromRef":{"displayId":"docs/wowtax-8238"},"toRef":{"displayId":"master"},
        "author":{"user":{"slug":"u_author"}},
        "links":{"self":[{"href":"https://server.com/projects/CORP/repos/tax-api/pull-requests/154"}]}}"#;

    /// Answers one request with `reply` as written, and hands the request back.
    ///
    /// The wait has a deadline: a client that refuses before sending — a server
    /// whose address is `http`, say — must fail a test, not hang it.
    fn serve(reply: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() > deadline {
                            return "<no request>".to_string();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => return "<no request>".to_string(),
                }
            };
            socket.set_nonblocking(false).unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            while let Ok(read @ 1..) = socket.read(&mut buffer) {
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| line.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            socket.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn json_reply(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn server_at(base_url: &str) -> CodeHostServer {
        CodeHostServer {
            id: "s".to_string(),
            base_url: base_url.to_string(),
            hosts: Vec::new(),
            trusted_cert_pem: None,
            token_set: true,
        }
    }

    /// The request the client sends for `body`'s answer, and what it made of it.
    fn ask<T>(reply: String, ask: impl Fn(&Call) -> Result<T, CodeHostError>) -> (T, String, String) {
        let (base, server) = serve(reply);
        let at = server_at(&base);
        let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();
        let call = Call { server: &at, token: "ATATT-secret", remote: &remote };
        let answer = ask(&call).expect("an answer");
        (answer, server.join().unwrap(), base)
    }

    /// Runs `ask` against a server that answers one request with `reply`, and
    /// hands back the request the client sent.
    fn with_server<T>(reply: &str, ask: impl Fn(&Call) -> T) -> (T, String) {
        let (base, server) = serve(reply.to_string());
        let at = server_at(&base);
        let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();
        let call = Call { server: &at, token: "ATATT-secret", remote: &remote };
        let answer = ask(&call);
        (answer, server.join().unwrap())
    }

    fn body_of(request: &str) -> Value {
        let (_, body) = request.split_once("\r\n\r\n").expect("a head and a body");
        serde_json::from_str(body).expect("a JSON body")
    }

    /// The probe's own answer, read back: the pull request of a branch with a
    /// `/` in its name, found through the encoded ref.
    #[test]
    fn the_pull_request_the_probe_saw_is_read_as_the_strip_shows_it() {
        let (pr, asked, _) = ask(json_reply("200 OK", PRS), |call| Bitbucket.pull_request(call, "docs/wowtax-8238"));
        let pr = pr.expect("a pull request");

        assert!(asked.starts_with("GET /rest/api/latest/projects/corp/repos/tax-api/pull-requests?"), "{asked}");
        assert!(asked.contains("at=refs%2Fheads%2Fdocs%2Fwowtax-8238"), "the branch's slash is not encoded: {asked}");
        assert!(asked.contains("direction=OUTGOING") && asked.contains("state=OPEN"), "{asked}");
        assert!(asked.to_lowercase().contains("authorization: bearer atatt-secret"), "{asked}");

        assert_eq!((pr.id, pr.version), (153, 6));
        assert_eq!((pr.author.as_str(), pr.from_branch.as_str(), pr.to_branch.as_str()), ("u_author", "docs/wowtax-8238", "master"));
        assert_eq!(pr.reviewers, [Reviewer { slug: "u_reviewer".to_string(), status: ReviewStatus::Unapproved }]);
        assert_eq!(pr.url, "https://server.com/projects/CORP/repos/tax-api/pull-requests/153");
    }

    /// An empty list is no pull request — Create PR, not an error.
    #[test]
    fn no_pull_request_for_the_branch_is_nothing_rather_than_a_failure() {
        let (pr, _, _) = ask(json_reply("200 OK", r#"{"size":0,"isLastPage":true,"values":[]}"#), |call| {
            Bitbucket.pull_request(call, "main")
        });
        assert_eq!(pr, None);
    }

    /// The three veto lines the probe's merge check returned, as the server
    /// wrote them and in its order.
    #[test]
    fn the_merge_check_keeps_the_server_s_own_lines() {
        let (merge, asked, _) = ask(json_reply("200 OK", MERGE), |call| Bitbucket.merge_check(call, 153));
        assert!(asked.starts_with("GET /rest/api/latest/projects/corp/repos/tax-api/pull-requests/153/merge"), "{asked}");
        assert!(!merge.can_merge && !merge.conflicted);
        assert_eq!(
            merge.vetoes,
            [
                "Include Code Quality detected missing project data",
                "Not all required builds are successful yet",
                "Requires approvals"
            ]
        );
    }

    /// The legacy path, because the repository's own builds endpoint wants a
    /// build key it is never given (the probe's 400).
    #[test]
    fn builds_are_read_from_the_legacy_path_and_an_unknown_state_is_not_a_success() {
        let (builds, asked, _) = ask(json_reply("200 OK", BUILDS), |call| {
            Bitbucket.builds(call, "94beed93512849804d03fe90d10a99b1418d4342")
        });
        assert!(asked.starts_with("GET /rest/build-status/latest/commits/94beed93512849804d03fe90d10a99b1418d4342"), "{asked}");
        assert_eq!(
            builds.iter().map(|build| build.state).collect::<Vec<_>>(),
            [BuildState::Successful, BuildState::Failed, BuildState::InProgress, BuildState::Unknown]
        );
        assert_eq!(builds[0].name, "build #12");

        let (none, _, _) = ask(json_reply("200 OK", BUILDS_EMPTY), |call| Bitbucket.builds(call, "94beed9"));
        assert!(none.is_empty());
    }

    #[test]
    fn a_branch_on_the_server_is_its_commit_and_a_missing_one_is_nothing() {
        let (commit, asked, _) = ask(json_reply("200 OK", BRANCHES), |call| Bitbucket.branch_commit(call, "docs/wowtax-8238"));
        assert!(asked.contains("filterText=docs%2Fwowtax-8238"), "{asked}");
        // Without it the exact name can be past the page among longer ones.
        assert!(asked.contains("boostMatches=true"), "{asked}");
        assert_eq!(commit.as_deref(), Some("94beed93512849804d03fe90d10a99b1418d4342"));

        // `filterText` is a substring search: a longer branch in its answer is
        // not this branch.
        let (missing, _, _) = ask(json_reply("200 OK", BRANCHES), |call| Bitbucket.branch_commit(call, "wowtax-8238"));
        assert_eq!(missing, None);
    }

    #[test]
    fn the_default_branch_and_the_clone_links_are_read() {
        let (branch, asked, _) = ask(json_reply("200 OK", DEFAULT_BRANCH), |call| Bitbucket.default_branch(call));
        assert!(asked.starts_with("GET /rest/api/latest/projects/corp/repos/tax-api/default-branch HTTP/1.1"), "{asked}");
        assert_eq!(branch, "master");

        let (links, _, _) = ask(json_reply("200 OK", REPO), |call| Bitbucket.clone_links(call));
        assert_eq!(
            links,
            ["ssh://git@server.com/corp/tax-api.git", "https://server.com/scm/corp/tax-api.git"]
        );
    }

    /// The slug comes from the header, before the body is read: it is what
    /// approve sends as the participant.
    #[test]
    fn who_the_token_belongs_to_is_the_x_ausername_header() {
        let (me, _, _) = ask(
            format!("HTTP/1.1 200 OK\r\nX-AUSERNAME: u_me\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"),
            |call| Bitbucket.me(call),
        );
        assert_eq!(me, "u_me");

        let (base, server) = serve(json_reply("200 OK", "{}"));
        let at = server_at(&base);
        let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();
        let call = Call { server: &at, token: "t", remote: &remote };
        let error = Bitbucket.me(&call).unwrap_err();
        server.join().unwrap();
        assert!(
            matches!(&error, CodeHostError::BadAnswer(message) if message.contains("X-AUSERNAME")),
            "{error}"
        );
    }

    /// The three statuses the strip acts on, and the server's own words for the
    /// rest — a 400 from a wrong project key is not "not a Bitbucket".
    #[test]
    fn statuses_become_the_errors_the_buttons_act_on() {
        // Each status through a server of its own.
        let stale = r#"{"errors":[{"exceptionName":"com.atlassian.bitbucket.pull.PullRequestOutOfDateException"}]}"#;
        for (status, body, wanted) in
            [("401 Unauthorized", "{}", "token"), ("403 Forbidden", "{}", "Repository write"), ("409 Conflict", stale, "read again")]
        {
            let (base, server) = serve(json_reply(status, body));
            let at = server_at(&base);
            let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();
            let call = Call { server: &at, token: "t", remote: &remote };
            let error = Bitbucket.default_branch(&call).unwrap_err().to_string();
            server.join().unwrap();
            assert!(error.contains(wanted), "{status}: {error}");
        }

        let (base, server) = serve(json_reply("400 Bad Request", r#"{"errors":[{"message":"Please specify a non-blank key"}]}"#));
        let at = server_at(&base);
        let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();
        let call = Call { server: &at, token: "t", remote: &remote };
        let error = Bitbucket.builds(&call, "94beed9").unwrap_err().to_string();
        server.join().unwrap();
        assert!(error.contains("400") && error.contains("non-blank key"), "{error}");
    }

    /// The reason the server carries a PEM: the app trusts the public roots or
    /// that file, never the keychain.
    #[test]
    fn a_server_behind_its_own_ca_is_trusted_with_its_pem_and_refused_without() {
        use crate::infra::http_agent::tests::{serve_tls_with, CA, LEAF, LEAF_KEY};

        let port = serve_tls_with(LEAF, LEAF_KEY, &json_reply("200 OK", DEFAULT_BRANCH));
        let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();

        let with_pem = CodeHostServer {
            id: "s".to_string(),
            base_url: format!("https://127.0.0.1:{port}"),
            hosts: Vec::new(),
            trusted_cert_pem: Some(CA.to_string()),
            token_set: true,
        };
        let call = Call { server: &with_pem, token: "t", remote: &remote };
        assert_eq!(Bitbucket.default_branch(&call).unwrap(), "master");

        let without = CodeHostServer { trusted_cert_pem: None, ..with_pem };
        let call = Call { server: &without, token: "t", remote: &remote };
        let error = Bitbucket.default_branch(&call).unwrap_err();
        assert!(matches!(error, CodeHostError::Tls(_)), "not a TLS refusal: {error}");
    }

    /// The three buttons' requests: the participant and its status, the version
    /// that is the optimistic lock, and the two refs a new pull request names.
    #[test]
    fn the_buttons_send_what_the_server_takes() {
        let pr = PullRequest {
            id: 153,
            version: 6,
            title: "a pull request".to_string(),
            author: "u_author".to_string(),
            from_branch: "docs/wowtax-8238".to_string(),
            to_branch: "master".to_string(),
            reviewers: Vec::new(),
            url: "https://server.com/projects/CORP/repos/tax-api/pull-requests/153".to_string(),
        };

        for (approved, wanted) in [(true, "APPROVED"), (false, "UNAPPROVED")] {
            let (answered, asked) =
                with_server(&json_reply("200 OK", "{}"), |call| Bitbucket.approve(call, &pr, "u_me", approved));
            answered.unwrap();
            assert!(
                asked.starts_with("PUT /rest/api/latest/projects/corp/repos/tax-api/pull-requests/153/participants/u_me HTTP/1.1"),
                "{asked}"
            );
            assert_eq!(body_of(&asked), json!({ "status": wanted }));
        }

        let (answered, asked) = with_server(&json_reply("200 OK", "{}"), |call| Bitbucket.merge(call, &pr));
        answered.unwrap();
        assert!(
            asked.starts_with("POST /rest/api/latest/projects/corp/repos/tax-api/pull-requests/153/merge?version=6 HTTP/1.1"),
            "{asked}"
        );
        // The resource takes JSON, and Atlassian's XSRF filter wants a client
        // that is not a browser to say so: without either a merge is 415 or 403.
        let head = asked.to_lowercase();
        assert!(head.contains("content-type: application/json"), "{asked}");
        assert!(head.contains("x-atlassian-token: no-check"), "{asked}");
        assert_eq!(body_of(&asked), json!({}));

        let (created, asked) =
            with_server(&json_reply("200 OK", CREATED), |call| Bitbucket.create(call, "docs/wowtax-8238", "master", "a title"));
        let created = created.expect("the new pull request");
        assert!(asked.starts_with("POST /rest/api/latest/projects/corp/repos/tax-api/pull-requests HTTP/1.1"), "{asked}");
        assert_eq!(
            body_of(&asked),
            json!({
                "title": "a title",
                "fromRef": { "id": "refs/heads/docs/wowtax-8238" },
                "toRef": { "id": "refs/heads/master" },
            })
        );
        assert_eq!((created.id, created.from_branch.as_str()), (154, "docs/wowtax-8238"));
    }

    /// A 409 is the optimistic lock only when the server says so; a vetoed
    /// merge and a second pull request between the same branches are 409 too,
    /// and their reason is what the user needs to see.
    #[test]
    fn only_a_stale_version_is_a_conflict() {
        let stale = r#"{"errors":[{"message":"You are attempting to modify a pull request based on out-of-date information.",
            "exceptionName":"com.atlassian.bitbucket.pull.PullRequestOutOfDateException"}]}"#;
        let vetoed = r#"{"errors":[{"message":"Merging the pull request has been vetoed.",
            "exceptionName":"com.atlassian.bitbucket.pull.PullRequestMergeVetoedException",
            "vetoes":[{"summaryMessage":"Requires approvals"}]}]}"#;
        let duplicate = r#"{"errors":[{"message":"Only one pull request may be open for a given source and target branch",
            "exceptionName":"com.atlassian.bitbucket.pull.DuplicatePullRequestException"}]}"#;

        assert_eq!(status_check(409, stale), Err(CodeHostError::Conflict));
        assert!(matches!(status_check(409, vetoed), Err(CodeHostError::Http(409, words)) if words.contains("vetoed")));
        assert!(matches!(status_check(409, duplicate), Err(CodeHostError::Http(409, words)) if words.contains("Only one")));
        assert!(matches!(status_check(409, "{}"), Err(CodeHostError::Http(409, _))));
    }

    /// `http` reaches this machine and nothing else, however the host is
    /// dressed: a name that starts like `localhost` is somebody's host.
    #[test]
    fn only_this_machine_is_loopback() {
        for local in ["http://localhost:7990", "http://127.0.0.1:1/x", "http://[::1]:8080", "http://LOCALHOST", "http://127.0.0.2"] {
            assert!(is_loopback(local), "{local}");
        }
        for remote in [
            "http://localhost.corp.example",
            "http://127.0.0.1.nip.io",
            "http://localhost@code.corp.example",
            "http://localhost:x@code.corp.example/",
            "http://code.corp.example/localhost",
            "http://",
        ] {
            assert!(!is_loopback(remote), "{remote}");
        }
    }

    /// A token goes over TLS or nowhere: a server stored as `http://` on a real
    /// host is refused before a single byte is sent.
    #[test]
    fn a_plain_http_server_gets_no_token() {
        let at = server_at("http://git.server.example");
        let remote = RemoteRepo::parse("ssh://git@server.com/corp/tax-api.git").unwrap();
        let call = Call { server: &at, token: "ATATT-secret", remote: &remote };
        let error = Bitbucket.default_branch(&call).unwrap_err();
        assert!(
            matches!(&error, CodeHostError::Insecure(base) if base == "http://git.server.example"),
            "{error}"
        );
    }

    /// A 302 is an answer, not a step: following it would take the token to
    /// whichever host it names, and Data Center answers one for something as
    /// ordinary as a sign-in page.
    #[test]
    fn a_redirect_is_not_followed() {
        let redirect = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let (answer, _) = with_server(redirect, |call| Bitbucket.default_branch(call));
        assert!(matches!(answer, Err(CodeHostError::Http(302, _))), "{answer:?}");
    }

    /// A server that names the pull request's page apart from its own link
    /// means it: `web` is opened, and `self` only when it is all there is.
    #[test]
    fn the_web_link_is_preferred_over_self() {
        let both: Pr = serde_json::from_str(
            r#"{"id":1,"links":{"self":[{"href":"https://server.com/self"}],"web":[{"href":"https://server.com/web"}]}}"#,
        )
        .unwrap();
        assert_eq!(both.into_pull_request().url, "https://server.com/web");

        let only_self: Pr = serde_json::from_str(r#"{"id":1,"links":{"self":[{"href":"https://server.com/self"}]}}"#).unwrap();
        assert_eq!(only_self.into_pull_request().url, "https://server.com/self");
    }

    /// The connect form's check, before a token is saved: no `Authorization`
    /// goes with it, and a server that is something else says so.
    #[test]
    fn identify_asks_without_a_token_and_knows_bitbucket_when_it_sees_it() {
        let answer = r#"{"version":"8.19.26","buildNumber":"8019026","buildDate":"1","displayName":"Bitbucket"}"#;
        let (said, asked) = with_server(&json_reply("200 OK", answer), |call| Bitbucket.identify(call.server));
        assert_eq!(said.unwrap(), "Bitbucket 8.19.26");
        assert!(asked.starts_with("GET /rest/api/latest/application-properties HTTP/1.1"), "{asked}");
        assert!(!asked.to_lowercase().contains("authorization"), "a token went with it: {asked}");

        let (other, _) =
            with_server(&json_reply("200 OK", r#"{"displayName":"Jira","version":"9"}"#), |call| Bitbucket.identify(call.server));
        assert!(matches!(&other, Err(CodeHostError::BadAnswer(said)) if said.contains("Jira")), "{other:?}");

        let insecure = Bitbucket.identify(&server_at("http://git.server.example"));
        assert!(matches!(insecure, Err(CodeHostError::Insecure(_))), "{insecure:?}");
    }

    /// A `NEEDS_WORK` is a decision the strip shows; a word this app has no
    /// word for is not an approval, and not a success either.
    #[test]
    fn the_states_this_app_has_no_word_for_are_neither_a_yes_nor_a_success() {
        assert_eq!(review_status("APPROVED"), ReviewStatus::Approved);
        assert_eq!(review_status("NEEDS_WORK"), ReviewStatus::NeedsWork);
        assert_eq!(review_status("UNAPPROVED"), ReviewStatus::Unapproved);
        assert_eq!(review_status("SOMETHING_NEW"), ReviewStatus::Unapproved);

        assert_eq!(build_state("SUCCESSFUL"), BuildState::Successful);
        assert_eq!(build_state("FAILED"), BuildState::Failed);
        assert_eq!(build_state("INPROGRESS"), BuildState::InProgress);
        assert_eq!(build_state("QUEUED"), BuildState::Unknown);
    }

    #[test]
    fn a_segment_is_encoded_as_the_api_takes_it() {
        assert_eq!(encode("docs/wowtax-8238"), "docs%2Fwowtax-8238");
        assert_eq!(encode("refs/heads/main"), "refs%2Fheads%2Fmain");
        assert_eq!(encode("corp-wowtax"), "corp-wowtax");
        assert_eq!(encode("a b&c"), "a%20b%26c");
    }

    #[test]
    fn a_refusal_says_its_own_words_cut_short() {
        assert_eq!(said(r#"{"errors":[{"message":"Repository not found"}]}"#), "Repository not found");
        assert_eq!(said("plain text"), "plain text");
        assert_eq!(said(&"x".repeat(ERROR_CHARS + 1)), format!("{}…", "x".repeat(ERROR_CHARS)));
    }
}
