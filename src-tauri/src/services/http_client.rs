//! The HTTP page's use cases over its two stores (`docs/45-http-client.md`):
//! a request sent and what came back kept, a Postman file imported into
//! whichever store it belongs to, and saved responses whose request is gone dropped.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde_json::Value;

use crate::domain::http_auth::{refusal, OAuth2, Token};
use crate::domain::http_client::{agent_hidden, credential_literals, hiding, AgentReply, AgentSave, AgentSaved, AgentSend, AgentSent, Environment, HistoryEntry, HttpChanged, HttpError, HttpEventSink, HttpStands, OpenStand, Outcome, Outgoing, Prepared, Reach, ResponseView, SavedResponse, Scope, StandView, Variable};
use crate::domain::http_scripts::{changes, overlaid, request_after, request_for_script, request_sent, response_for, ScriptReport, ScriptVars};
use crate::domain::postman::{environment_to_postman, read_import, script_hash, Auth, Collection, Imported};
use crate::infra::http_client::NoCookies;
use crate::infra::http_cookies::StoredJar;
use crate::infra::postman_sandbox::{Ran, Sandbox, LIMITS};
use crate::infra::{chat_store, http_agent, http_client, http_collections, http_env_store, settings_store};

/// Sends a collection's request — `edited`, the page's unsaved `request`, in
/// place of the stored one — into `environment`. What came back is on disk, as
/// the request's last response and in history, before the window gets it.
/// A variable found nowhere fails here, before anything is sent. So does a
/// pre-request script that fails: what it was for — a token, say — did not happen.
/// What scripts wrote is kept either way.
pub fn send(collection_id: &str, request_id: &str, edited: Option<Value>, environment: Option<&str>) -> Result<ResponseView, HttpError> {
    let mut collection = http_collections::get(collection_id)?;
    if let Some(edited) = edited {
        let item = collection.item_mut(request_id).ok_or_else(|| HttpError::NoSuchItem(request_id.into()))?;
        item.insert("request".into(), edited);
    }
    let environment = environment.map(http_env_store::get).transpose()?;
    // The last to start, not the last past its scripts: one can wait a minute there.
    let key = format!("{collection_id}/{request_id}");
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    latest().insert(key.clone(), ticket);
    let (sent, report, _) = run(&mut collection, collection_id, request_id, environment.as_ref(), Vec::new(), None)?;
    let saved = SavedResponse { at: sent.at, environment: sent.environment, outcome: sent.outcome, body: sent.body, scripts: ScriptReport::default() };
    save(collection_id, request_id, Sent { key, ticket, saved }, report)
}

/// An agent's send: the chat that signs it in history, and the names the
/// collections write where a credential goes — its mask hides them besides secrets.
struct ForAgent {
    chat: String,
    credentials: BTreeSet<String>,
    /// Header values and the query the model wrote: a wrong token it sends is its to read back.
    written: Vec<String>,
}

/// A request through its scripts, with `local` over the other variables, and what
/// they wrote kept. What came back, the scripts' report under the send's mask, and the mask.
fn run(collection: &mut Collection, collection_id: &str, request_id: &str, environment: Option<&Environment>, local: Vec<Variable>, agent: Option<ForAgent>) -> Result<(Exchanged, ScriptReport, Scope), HttpError> {
    let values = http_env_store::collection_values()?;
    let before = ScriptVars {
        globals: http_env_store::globals()?,
        collection: overlaid(collection.variables(), values.get(collection_id).map(Vec::as_slice).unwrap_or_default()),
        environment: environment.map(|e| e.variables.clone()).unwrap_or_default(),
        local,
    };
    let mut report = ScriptReport::default();
    let mut scripts = Scripts::new(collection, collection_id, request_id, environment, before.clone(), &mut report, agent)?;
    let sent = exchange(collection, collection_id, request_id, environment, &mut scripts);
    let mask = scripts.mask();
    let after = scripts.vars;
    match write_variables(&before, &after, collection_id, environment) {
        Ok(changed) => report.changed = changed,
        Err(e) => {
            report.error.get_or_insert(format!("What the scripts wrote was not kept: {e}"));
        }
    }
    let sent = sent.map_err(|e| match e {
        HttpError::Script(why) => HttpError::Script(mask.hide(&why)),
        HttpError::OAuth2(why) => HttpError::OAuth2(mask.hide(&why)),
        other => other,
    })?;
    let report = report.hidden(&mask);
    Ok((sent, report, mask))
}

/// What came back of a send, its secrets hidden but in the body.
struct Exchanged {
    at: i64,
    environment: Option<String>,
    outcome: Outcome,
    body: Vec<u8>,
}

/// A request through its scripts, and what came back with its secrets hidden.
fn exchange(collection: &mut Collection, collection_id: &str, request_id: &str, environment: Option<&Environment>, scripts: &mut Scripts) -> Result<Exchanged, HttpError> {
    scripts.before_sending(collection, request_id)?;
    let cert = environment.and_then(|e| e.trusted_cert.as_deref());
    let scope = scripts.vars.scope();
    let held = oauth_token(collection, collection_id, request_id, &scope, cert)?;
    if let Some(held) = &held {
        scripts.keep_secret(&held.access);
    }
    let mut prepared = collection.prepare_with(request_id, &scope, held.as_ref().map(|h| h.access.as_str()))?;
    let page = scripts.agent.is_none();
    let (mut outcome, mut body) = deliver(&prepared.sent, cert, page);
    // Once: a token just got that is refused is not one the next will fix.
    if let Some(stale) = held.filter(|h| !h.fresh && matches!(outcome, Outcome::Response { status: 401, .. })) {
        match renewed(&stale, collection_id, cert) {
            Ok(access) => {
                scripts.keep_secret(&access);
                prepared = collection.prepare_with(request_id, &scope, Some(&access))?;
                (outcome, body) = deliver(&prepared.sent, cert, page);
            }
            // The 401 stands, and says why the next request will meet one too.
            Err(e) => {
                scripts.report.error.get_or_insert(e.to_string());
            }
        }
    }
    let at = chrono::Utc::now().timestamp_millis();
    let name = environment.map(|e| e.name.clone());
    let written = collection.written_url(request_id).unwrap_or_default();
    let wrote = |literal: &String| scripts.agent.as_ref().is_some_and(|a| a.written.iter().any(|w| w.contains(literal.as_str())));
    let literals: Vec<String> = credential_literals(&prepared.sent).into_iter().filter(|l| !wrote(l)).collect();
    scripts.kept.extend(literals);
    let mask = scripts.mask();
    match scripts.agent.as_ref().map(|a| a.chat.clone()) {
        None => remember(at, name.clone(), None, written, Some(request_id), &prepared, &outcome, &body, &mask),
        // What the model was told, and no more.
        Some(chat) => {
            let told = AgentReply::new(outcome.clone(), &body, &mask);
            remember(at, name.clone(), Some(chat), written, Some(request_id), &prepared, &told.outcome, &told.body, &mask);
        }
    }
    scripts.after_response(&prepared, &outcome, &body);
    // What the test scripts learned counts too: a token they stored is a secret's value.
    let outcome = outcome.hidden(&scripts.mask());
    Ok(Exchanged { at, environment: name, outcome, body })
}

/// Through the sender, with the jar: what a send and a script's `pm.sendRequest` have
/// in common. An agent's own request does not `follow` a redirect: where it leads is the
/// stand's to say, and the model's to read.
fn deliver(request: &Outgoing, cert: Option<&str>, page: bool) -> (Outcome, Vec<u8>) {
    if page { http_client::send(request, cert, &StoredJar) } else { http_client::send_once(request, cert, &NoCookies) }
}

/// The token a send goes with, and what it would take to get another.
struct Held {
    oauth: OAuth2,
    key: String,
    access: String,
    /// Got for this send: a 401 is not about its age.
    fresh: bool,
}

/// Held from reading a token to storing the next: two sends at once would each
/// spend the one `refresh_token`, and the second would be refused.
// ponytail: one lock for all collections, so a token endpoint that hangs holds every `oauth2` send for up to a minute; per key if that is ever met.
fn renewing() -> MutexGuard<'static, ()> {
    static RENEWING: Mutex<()> = Mutex::new(());
    RENEWING.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The token for a request's `oauth2`: the one kept, renewed if it is past its time; else
/// the one the file saved; else a new one. None where Kibo cannot get one: the file's goes as it is.
fn oauth_token(collection: &Collection, collection_id: &str, request_id: &str, scope: &Scope, cert: Option<&str>) -> Result<Option<Held>, HttpError> {
    let Auth::OAuth2(oauth) = collection.auth_for(request_id)? else { return Ok(None) };
    // With an empty token, only what was written by hand is left: that stands, and the client's secret stays home.
    let written = collection.prepare_with(request_id, scope, Some(""))?.sent.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("authorization"));
    if written && !oauth.in_query {
        return Ok(None);
    }
    if !oauth.can_fetch() {
        if oauth.access_token.is_empty() && !oauth.grant.is_empty() {
            return Err(HttpError::OAuth2(oauth.why_no_token()));
        }
        return Ok(None);
    }
    let oauth = oauth.filled(scope)?;
    let key = oauth.key();
    let _held = renewing();
    let (access, fresh) = match http_env_store::token(collection_id, &key)? {
        Some(token) if !token.expired(now()) => (token.access, false),
        Some(token) => (renew(&oauth, &key, collection_id, Some(token), cert)?, true),
        None => match scope.fill(&oauth.access_token).map(|f| f.sent).ok().filter(|t| !t.is_empty()) {
            Some(saved) => (saved, false),
            None => (renew(&oauth, &key, collection_id, None, cert)?, true),
        },
    };
    Ok(Some(Held { oauth, key, access, fresh }))
}

/// After a 401: another send may have renewed it already, and that one is good.
fn renewed(stale: &Held, collection_id: &str, cert: Option<&str>) -> Result<String, HttpError> {
    let _held = renewing();
    let kept = http_env_store::token(collection_id, &stale.key)?;
    if let Some(token) = kept.as_ref().filter(|t| t.access != stale.access) {
        return Ok(token.access.clone());
    }
    renew(&stale.oauth, &stale.key, collection_id, kept, cert)
}

/// A token after `old`, by its `refresh_token` while that is good, else by the grant — and kept.
fn renew(oauth: &OAuth2, key: &str, collection_id: &str, old: Option<Token>, cert: Option<&str>) -> Result<String, HttpError> {
    let refresh = old.and_then(|t| t.refresh);
    let refreshed = refresh.as_deref().map(|r| ask_token(&oauth.refresh_request(r), Some(r), cert));
    let token = match refreshed {
        Some(Ok(token)) => token,
        _ => ask_token(&oauth.token_request(), None, cert)?,
    };
    let access = token.access.clone();
    http_env_store::save_token(collection_id, key, token)?;
    Ok(access)
}

/// Not through history: its body holds the client's secret, and its reply the token.
fn ask_token(request: &Outgoing, refresh_before: Option<&str>, cert: Option<&str>) -> Result<Token, HttpError> {
    match http_client::send(request, cert, &NoCookies) {
        (Outcome::Response { status: 200..=299, .. }, body) => Token::from_reply(&body, now(), refresh_before),
        (Outcome::Response { status, .. }, body) => Err(refusal(status, &body)),
        (Outcome::Failed { error }, _) => Err(HttpError::OAuth2(format!("the token endpoint: {error}"))),
    }
}

/// A request that went out, on its way to the store.
struct Sent {
    key: String,
    ticket: u64,
    saved: SavedResponse,
}

/// A store that fails now must not hide what came back: the request went out.
#[allow(clippy::too_many_arguments)]
fn remember(at: i64, environment: Option<String>, agent_chat: Option<String>, written: String, request_id: Option<&str>, prepared: &Prepared, outcome: &Outcome, body: &[u8], scope: &Scope) {
    let mut entry = HistoryEntry::new(at, environment, agent_chat, scope.hide(&written), &prepared.hidden(scope), outcome.clone().hidden(scope), body);
    entry.request_id = request_id.map(str::to_string);
    if let Err(e) = http_collections::record(entry) {
        eprintln!("http: a send not written to history: {e}");
    }
}

/// Whether a variable was written.
fn write_variables(before: &ScriptVars, after: &ScriptVars, collection_id: &str, environment: Option<&Environment>) -> Result<bool, HttpError> {
    let (globals, values, own) = (changes(&before.globals, &after.globals), changes(&before.collection, &after.collection), changes(&before.environment, &after.environment));
    let own = environment.map(|e| (e.id.as_str(), own.as_slice())).filter(|(_, c)| !c.is_empty());
    if globals.is_empty() && values.is_empty() && own.is_none() {
        return Ok(false);
    }
    http_env_store::write_changes(own, &globals, (collection_id, &values))?;
    Ok(true)
}

/// A send's scripts: those turned on, of the collection, the folders down and the request.
struct Scripts<'a> {
    /// The variables as the send found them, and as the scripts have left them.
    start: ScriptVars,
    vars: ScriptVars,
    report: &'a mut ScriptReport,
    environment: Option<&'a Environment>,
    /// What runs before the request and after it, in order.
    before: Vec<String>,
    after: Vec<String>,
    sandbox: Option<Sandbox>,
    name: String,
    /// An agent's send, masked by the agent's rules.
    agent: Option<ForAgent>,
    /// What went as a credential, for an agent's mask: the collection can write one out.
    kept: Vec<String>,
}

impl<'a> Scripts<'a> {
    fn new(collection: &Collection, id: &str, request_id: &str, environment: Option<&'a Environment>, vars: ScriptVars, report: &'a mut ScriptReport, agent: Option<ForAgent>) -> Result<Scripts<'a>, HttpError> {
        // Only the scripts turned on run; the others are said, by where they are.
        let on = scripts_on(id, collection);
        let mut off = Vec::new();
        let mut pick = |listen: &str, when: &str| -> Result<Vec<String>, HttpError> {
            let (run, not): (Vec<_>, Vec<_>) = collection.scripts(request_id, listen)?.into_iter().partition(|(_, code)| on.contains(&script_hash(code)));
            off.extend(not.into_iter().map(|(place, _)| format!("{when} of {place}")));
            Ok(run.into_iter().map(|(_, code)| code).collect())
        };
        let (before, after) = (pick("prerequest", "pre-request")?, pick("test", "post-response")?);
        report.off = off;
        let name = collection.item_name(request_id).unwrap_or_default();
        Ok(Scripts { start: vars.clone(), vars, report, environment, before, after, sandbox: None, name, agent, kept: Vec::new() })
    }

    /// Every secret, before the scripts and after; for an agent, all it may not read too.
    fn mask(&self) -> Scope {
        let Some(agent) = &self.agent else { return ScriptVars::mask(&[&self.start, &self.vars]) };
        let layers = |v: &'_ ScriptVars| [v.globals.clone(), v.collection.clone(), v.environment.clone(), v.local.clone()];
        let (start, now) = (layers(&self.start), layers(&self.vars));
        let all: Vec<&[Variable]> = start.iter().chain(now.iter()).map(Vec::as_slice).collect();
        let hidden = agent_hidden(&all, &agent.credentials);
        let marked = |[globals, collection, environment, local]: [Vec<Variable>; 4]| ScriptVars {
            globals: hiding(&globals, &hidden),
            collection: hiding(&collection, &hidden),
            environment: hiding(&environment, &hidden),
            local: hiding(&local, &hidden),
        };
        ScriptVars::mask(&[&marked(start.clone()), &marked(now.clone())]).keeping(self.kept.iter().map(String::as_str))
    }

    /// A value no variable holds, a token got for this send, masked as a secret's is.
    fn keep_secret(&mut self, value: &str) {
        self.start.local.push(Variable { key: String::new(), value: value.into(), secret: true, enabled: true });
    }

    /// One script over the variables as the last one left them.
    fn run(&mut self, listen: &str, code: &str, request: &Value, response: Option<&Value>) -> Result<Ran, HttpError> {
        if self.sandbox.is_none() {
            self.sandbox = Some(Sandbox::boot(LIMITS)?);
        }
        let (scope, mask) = (self.vars.scope(), self.mask());
        let environment = self.environment;
        let context = self.vars.context(request, response);
        let sandbox = self.sandbox.as_ref().expect("booted above");
        let ran = sandbox.run(listen, code, &self.name, context, &mut |sub| sub_request(sub, &scope, &mask, environment))?;
        self.vars.absorb(&ran.result);
        self.report.tests.extend(ran.tests.iter().cloned());
        self.report.console.extend(ran.console.iter().cloned());
        Ok(ran)
    }

    /// The pre-request scripts, each changing the request the next one gets.
    fn before_sending(&mut self, collection: &mut Collection, request_id: &str) -> Result<(), HttpError> {
        for code in std::mem::take(&mut self.before) {
            let item = collection.item_mut(request_id).ok_or_else(|| HttpError::NoSuchItem(request_id.into()))?;
            let request = item.get("request").cloned().unwrap_or_default();
            let ran = self.run("prerequest", &code, &request_for_script(&request), None)?;
            if let Some(after) = ran.result.get("request") {
                item.insert("request".into(), request_after(&request, after));
            }
            if let Some(error) = ran.error {
                return Err(HttpError::Script(format!("A pre-request script failed, and nothing was sent: {error}")));
            }
            if ran.skip_request {
                return Err(HttpError::Script("A pre-request script skipped the request.".into()));
            }
        }
        Ok(())
    }

    /// The test scripts, when there is a response to test. One that fails is said
    /// beside the response, which stands; the rest still run, as in Postman.
    fn after_response(&mut self, prepared: &Prepared, outcome: &Outcome, body: &[u8]) {
        let Outcome::Response { status, headers, elapsed_ms, .. } = outcome else { return };
        let (request, response) = (request_sent(&prepared.sent), response_for(*status, headers, body, *elapsed_ms));
        for code in std::mem::take(&mut self.after) {
            match self.run("test", &code, &request, Some(&response)) {
                Ok(ran) => {
                    if let Some(error) = ran.error {
                        self.report.error.get_or_insert(error);
                    }
                }
                // Past a limit the sandbox is not used again.
                Err(e) => {
                    self.report.error = Some(e.to_string());
                    return;
                }
            }
        }
    }
}

/// `pm.sendRequest`: the same sender and the same history as the page's own requests.
fn sub_request(request: &Value, scope: &Scope, mask: &Scope, environment: Option<&Environment>) -> Result<Value, HttpError> {
    let request = request_after(&Value::Null, request);
    let written = request.get("url").map(|u| u.get("raw").unwrap_or(u)).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut one = Collection::new("sub-request", "sub-request");
    one.add(None, serde_json::json!({"id": "sub-request", "name": "sub-request", "request": request}))?;
    let prepared = one.prepare("sub-request", scope)?;
    let (outcome, body) = deliver(&prepared.sent, environment.and_then(|e| e.trusted_cert.as_deref()), true);
    let at = chrono::Utc::now().timestamp_millis();
    remember(at, environment.map(|e| e.name.clone()), None, written, None, &prepared, &outcome, &body, mask);
    match outcome {
        Outcome::Response { status, headers, elapsed_ms, .. } => Ok(response_for(status, &headers, &body, elapsed_ms)),
        Outcome::Failed { error } => Err(HttpError::Script(error)),
    }
}

/// The environments as one chat's agent reaches them. History keeps what the
/// model read, so what the page shows of an agent's send is what it was told.
pub struct Stands {
    pub changed: HttpEventSink,
    pub chat: String,
}

impl HttpStands for Stands {
    fn open(&self) -> Result<Vec<OpenStand>, HttpError> {
        let open = http_env_store::list()?.into_iter().filter(|e| !e.agent.is_off());
        Ok(open.map(|e| OpenStand { id: e.id, name: e.name, access: e.agent }).collect())
    }

    fn describe(&self, id: &str) -> Result<StandView, HttpError> {
        let environment = http_env_store::get(id)?;
        environment.described(&reach(&environment)?)
    }

    fn sending(&self, id: &str, send: &AgentSend) -> Result<(String, String), HttpError> {
        let environment = http_env_store::get(id)?;
        let plan = with_collections(|collections, globals, _| environment.agent_send(collections, globals, send))??;
        Ok((plan.acts, format!("{} {}, from \"{}\"", plan.method, plan.url, plan.name)))
    }

    /// The page's Send of a saved request with the model's edits (HC-10c): its auth,
    /// `oauth2` and the scripts turned on. The environment and the collections are read
    /// again: access taken back, or a request deleted, mid-turn counts from the next
    /// call. History gets it signed by the chat; the request's last response stays the user's.
    fn send(&self, id: &str, send: &AgentSend) -> Result<AgentSent, HttpError> {
        let environment = http_env_store::get(id)?;
        let (plan, collection_id) = with_collections(|collections, globals, ids| environment.agent_send(collections, globals, send).map(|plan| {
            let id = ids[plan.collection].to_string();
            (plan, id)
        }))??;
        let request_id = send.request.trim();
        let mut collection = http_collections::get(&collection_id)?;
        let item = collection.item_mut(request_id).ok_or_else(|| HttpError::NoRequest(request_id.into()))?;
        item.insert("request".into(), plan.request);
        let chat = chat_store::title(&self.chat).ok().flatten().unwrap_or_else(|| self.chat.clone());
        let local = send.variables.iter().map(|(key, value)| Variable { key: key.clone(), value: value.clone(), secret: false, enabled: true }).collect();
        let written = send.edits.headers.iter().filter_map(|(_, v)| v.clone()).chain(send.edits.query.clone()).collect();
        let agent = ForAgent { chat, credentials: plan.credentials, written };
        let ran = run(&mut collection, &collection_id, request_id, Some(&environment), local, Some(agent));
        (self.changed)(HttpChanged);
        let (sent, scripts, mask) = ran?;
        Ok(AgentSent { method: plan.method, url: mask.hide(&plan.url), reply: AgentReply::new(sent.outcome, &sent.body, &mask), scripts })
    }

    /// A case kept (HC-10e): the new request in the template's collection, beside it
    /// or in the folder named inside the template's own, made when there is none. Only
    /// added: nothing of the user's is changed, and no script is turned on — the case
    /// has none of its own. Where it goes is read from the file as it is written: a
    /// folder made or the template moved meanwhile counts.
    fn save(&self, id: &str, save: &AgentSave) -> Result<AgentSaved, HttpError> {
        let environment = http_env_store::get(id)?;
        let (plan, collection_id) = with_collections(|collections, globals, ids| environment.agent_save(collections, globals, save).map(|plan| {
            let id = ids[plan.collection].to_string();
            (plan, id)
        }))??;
        let place = change(&collection_id, |c| {
            let (parent, new_folder, place) = c.case_place(save.request.trim(), save.folder.as_deref())?;
            let parent = match new_folder {
                Some(name) => {
                    let folder = uuid::Uuid::new_v4().to_string();
                    c.add(parent.as_deref(), serde_json::json!({"id": folder, "name": name, "item": []}))?;
                    Some(folder)
                }
                None => parent,
            };
            c.add(parent.as_deref(), plan.item.clone())?;
            Ok(place)
        })?;
        (self.changed)(HttpChanged);
        Ok(AgentSaved { id: plan.id, place, method: plan.method, url: plan.url })
    }

    fn read(&self, id: &str, request: &str) -> Result<String, HttpError> {
        let environment = http_env_store::get(id)?;
        with_collections(|collections, globals, _| environment.read_request(collections, globals, request))?
    }
}

/// Where the user's collections go with `environment`, each filled in as the page
/// sends it.
fn reach(environment: &Environment) -> Result<Reach, HttpError> {
    with_collections(|collections, globals, _| Reach::new(collections, globals, environment))
}

/// The user's collections, each with its variables as a send sees them, globals,
/// and each collection's id. A collection whose file does not read is left out.
fn with_collections<T>(f: impl FnOnce(&[(&Collection, Vec<Variable>)], &[Variable], &[&str]) -> T) -> Result<T, HttpError> {
    let (globals, values) = (http_env_store::globals()?, http_env_store::collection_values()?);
    let all = http_collections::all()?;
    let read: Vec<(&str, &Collection)> = all.iter().filter_map(|(id, read)| read.as_ref().ok().map(|c| (id.as_str(), c))).collect();
    let collections: Vec<(&Collection, Vec<Variable>)> = read.iter().map(|(id, c)| (*c, overlaid(c.variables(), values.get(*id).map(Vec::as_slice).unwrap_or_default()))).collect();
    let ids: Vec<&str> = read.iter().map(|(id, _)| *id).collect();
    Ok(f(&collections, &globals, &ids))
}

/// The hashes of collection `id`'s scripts turned on (HC-11a). Trust given before to
/// the whole collection carries over once: the scripts unchanged since, all of them
/// are on; changed, none. Settings that cannot be read turn nothing on.
fn scripts_on(id: &str, collection: &Collection) -> BTreeSet<String> {
    let settings = settings_store::load().unwrap_or_default();
    if !settings.http_script_trust.contains_key(id) {
        return settings.http_scripts_on.get(id).cloned().unwrap_or_default();
    }
    let carried = settings_store::update(|s| {
        if let Some(old) = s.http_script_trust.remove(id) {
            if collection.scripts_digest().is_some_and(|d| d.hash == old) {
                s.http_scripts_on.entry(id.to_string()).or_default().extend(collection.all_scripts().into_iter().flat_map(|l| l.hashes));
            }
        }
        s.http_scripts_on.get(id).cloned().unwrap_or_default()
    });
    carried.unwrap_or_default()
}

/// One level's scripts at one moment, for the Scripts tab: where, when, the hashes
/// to turn them on or off by, and whether all of them are on.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptState {
    /// The folder's or request's id; `None` for the collection's own.
    pub item: Option<String>,
    pub listen: String,
    /// The code as the send runs it — what the Scripts tab shows, so that what the user
    /// reads is what the switch turns on — with each one's hash in `hashes`.
    pub codes: Vec<String>,
    pub hashes: Vec<String>,
    pub on: bool,
}

pub fn script_states(collection: &str) -> Result<Vec<ScriptState>, HttpError> {
    let c = http_collections::get(collection)?;
    let on = scripts_on(collection, &c);
    Ok(c.all_scripts().into_iter().map(|l| ScriptState { on: l.hashes.iter().all(|h| on.contains(h)), item: l.item, listen: l.listen, codes: l.codes, hashes: l.hashes }).collect())
}

/// Scripts turned on or off by the hashes the window showed (HC-11a). A hash none of
/// the collection's scripts has now is refused — they changed since they were shown —
/// and the hashes of scripts gone are dropped.
pub fn set_scripts(collection: &str, hashes: &[String], on: bool) -> Result<(), HttpError> {
    // Under the edits' lock: a script written meanwhile is either before this or after it.
    let _held = writing();
    let c = http_collections::get(collection)?;
    scripts_on(collection, &c);
    let present: HashSet<String> = c.all_scripts().into_iter().flat_map(|l| l.hashes).collect();
    if hashes.is_empty() || hashes.iter().any(|h| !present.contains(h)) {
        return Err(HttpError::Script("The collection's scripts changed — look at them again.".into()));
    }
    settings_store::update(|s| {
        let set = s.http_scripts_on.entry(collection.to_string()).or_default();
        set.retain(|h| present.contains(h));
        for h in hashes {
            if on {
                set.insert(h.clone());
            } else {
                set.remove(h);
            }
        }
    })
    .map_err(|e| HttpError::Store(e.to_string()))
}

fn save(collection_id: &str, request_id: &str, sent: Sent, report: ScriptReport) -> Result<ResponseView, HttpError> {
    let Sent { key, ticket, saved } = sent;
    let saved = SavedResponse { scripts: report, ..saved };
    // Held through the write, so a later send that ends first is not overwritten by this one.
    let latest = latest();
    if latest.get(&key) == Some(&ticket) {
        if let Err(e) = http_collections::save_response(collection_id, request_id, &saved) {
            eprintln!("http: a response not saved: {e}");
        }
    }
    drop(latest);
    Ok(saved.into())
}

/// Sends numbered as they start; and the last to start, per `<collection>/<request>`.
/// The page shows the last send's response (decision 3), so it is the one kept.
// ponytail: one lock for every request's save — per-request locks if many save at once.
static NEXT: AtomicU64 = AtomicU64::new(0);
static LATEST: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Mutex::default);

fn latest() -> MutexGuard<'static, HashMap<String, u64>> {
    LATEST.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The response a request's tab shows on opening; none before its first send.
pub fn last_response(collection_id: &str, request_id: &str) -> Result<Option<ResponseView>, HttpError> {
    Ok(http_collections::last_response(collection_id, request_id)?.map(ResponseView::from))
}

/// What an Import brought in, for the page to open it.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ImportedAs {
    Collection { id: String, name: String },
    Environment { id: String, name: String },
    Globals { count: usize },
}

pub fn import(text: &str) -> Result<ImportedAs, HttpError> {
    match read_import(text)? {
        Imported::Collection(mut collection) => {
            let taken = http_collections::all()?.into_iter().filter_map(|(_, c)| c.ok()).flat_map(|c| c.ids()).collect();
            collection.ensure_ids_besides(&taken);
            Ok(ImportedAs::Collection { id: http_collections::add(&collection)?, name: collection.name() })
        }
        Imported::Environment(environment) => {
            let done = ImportedAs::Environment { id: environment.id.clone(), name: environment.name.clone() };
            http_env_store::add(environment)?;
            Ok(done)
        }
        Imported::Globals(globals) => {
            let count = globals.len();
            http_env_store::set_globals(globals)?;
            Ok(ImportedAs::Globals { count })
        }
    }
}

/// A collection as the window gets it: the file's tree, or why it does not read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionView {
    pub id: String,
    pub file: Option<Value>,
    pub error: Option<String>,
    /// What scripts wrote to its variables, which lie over the file's.
    pub current: Vec<Variable>,
}

/// Every collection, as the page opens — when the responses of requests no
/// collection has any more go: one imported again with other ids leaves the
/// old ids' responses behind.
pub fn collections() -> Result<Vec<CollectionView>, HttpError> {
    let all = http_collections::all()?;
    let known: HashMap<String, Option<HashSet<String>>> =
        all.iter().map(|(id, collection)| (id.clone(), collection.as_ref().ok().map(|c| c.request_ids().into_iter().collect()))).collect();
    if let Err(e) = http_collections::prune_responses(&known) {
        eprintln!("http: saved responses not pruned: {e}");
    }
    // Sealed apart: a store that does not open must not take the collections with it.
    let mut values = http_env_store::collection_values().unwrap_or_else(|e| {
        eprintln!("http: script values not read: {e}");
        Default::default()
    });
    Ok(all
        .into_iter()
        .map(|(id, collection)| {
            let current = values.remove(&id).unwrap_or_default();
            match collection {
                Ok(c) => {
                    let mine = c.variables();
                    let shown = current.iter().map(|v| Variable { secret: mine.iter().any(|m| m.key == v.key && m.secret), ..v.clone() }.for_window()).collect();
                    CollectionView { id, file: Some(c.json().clone()), error: None, current: shown }
                }
                Err(e) => CollectionView { id, file: None, error: Some(e.to_string()), current: Vec::new() },
            }
        })
        .collect())
}

pub fn new_collection(name: &str) -> Result<CollectionView, HttpError> {
    let id = uuid::Uuid::new_v4().to_string();
    let collection = Collection::new(&id, name);
    http_collections::save(&id, &collection)?;
    Ok(CollectionView { id, file: Some(collection.json().clone()), error: None, current: Vec::new() })
}

/// Held while a collection file is written or deleted.
// ponytail: one lock for every collection — per-collection locks if writes ever queue behind each other.
fn writing() -> MutexGuard<'static, ()> {
    static WRITING: Mutex<()> = Mutex::new(());
    WRITING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A stored collection read, changed and written, one change at a time: two
/// edits of one file at once would each write it without the other's.
fn change<T>(id: &str, edit: impl FnOnce(&mut Collection) -> Result<T, HttpError>) -> Result<T, HttpError> {
    let _held = writing();
    let mut collection = http_collections::get(id)?;
    let done = edit(&mut collection)?;
    http_collections::save(id, &collection)?;
    Ok(done)
}

/// Under the edits' lock: one that read the file before would write it back after.
pub fn delete_collection(id: &str) -> Result<(), HttpError> {
    let _held = writing();
    http_collections::delete(id)?;
    http_env_store::forget_collection(id)?;
    settings_store::update(|s| {
        s.http_script_trust.remove(id);
        s.http_scripts_on.remove(id);
    })
    .map_err(|e| HttpError::Store(e.to_string()))?;
    Ok(())
}

/// The page's edit of a request, the rest of the file as it was.
pub fn save_request(collection: &str, request_id: &str, request: Value) -> Result<(), HttpError> {
    change(collection, |c| c.set_request(request_id, request))
}

/// The request's own script of one kind written from the page (HC-11b). Decision 5: an
/// edit of a script on keeps it on; anything else — one off, or none there — waits for
/// the switch. "None there" does not turn on: the page writes a moment after each pause,
/// so a script cleared and put back by undo would arrive as new, and foreign code that
/// was off would come back on. Only the page calls this: nothing the agent has writes a
/// script.
pub fn save_script(collection: &str, request_id: &str, listen: &str, code: &str) -> Result<(), HttpError> {
    if !matches!(listen, "prerequest" | "test") {
        return Err(HttpError::Script(format!("No such script: {listen}.")));
    }
    change(collection, |c| {
        let on = scripts_on(collection, c);
        let old = c.set_script(request_id, listen, code)?;
        let keep_on = !old.is_empty() && old.iter().all(|code| on.contains(&script_hash(code)));
        let present: HashSet<String> = c.all_scripts().into_iter().flat_map(|l| l.hashes).collect();
        // Under the edits' lock, which a switch takes too: neither sees the file and the
        // settings half done. The code of each pause before is dropped, so a script off that
        // comes to read like it is not on by that. Written before the file: if the file then
        // fails, the hash names code nowhere, and the next change drops it.
        settings_store::update(|s| {
            let set = s.http_scripts_on.entry(collection.to_string()).or_default();
            if keep_on {
                set.insert(script_hash(code));
            }
            set.retain(|h| present.contains(h));
        })
        .map_err(|e| HttpError::Store(e.to_string()))
    })
}

pub fn add_item(collection: &str, parent: Option<&str>, item: Value) -> Result<(), HttpError> {
    change(collection, |c| c.add(parent, item))
}

/// The collection itself when `id` is its own, else its folder or request.
pub fn rename(collection: &str, id: &str, name: &str) -> Result<(), HttpError> {
    change(collection, |c| {
        if id == collection {
            c.set_name(name);
        } else {
            c.item_mut(id).ok_or_else(|| HttpError::NoSuchItem(id.into()))?.insert("name".into(), name.into());
        }
        Ok(())
    })
}

/// A folder or request, with all it holds, moved last into the folder `parent` of the
/// collection `to`, or into `to` itself. Into another collection its requests' last
/// responses go with it; the target is written first, so a failure between the two
/// writes leaves the item in both rather than in neither.
pub fn move_item(from: &str, id: &str, to: &str, parent: Option<&str>) -> Result<(), HttpError> {
    if from == to {
        // A folder into itself or one of its own finds no `parent` once taken, and nothing is written.
        return change(from, |c| {
            let item = c.take(id)?;
            c.put(parent, item).map(drop)
        });
    }
    let _held = writing();
    let (mut source, mut target) = (http_collections::get(from)?, http_collections::get(to)?);
    let requests = target.put(parent, source.take(id)?)?;
    http_collections::save(to, &target)?;
    http_collections::save(from, &source)?;
    for request in requests {
        // Left behind, it goes when the page next opens, as a deleted request's does.
        if let Err(e) = http_collections::move_response(from, to, &request) {
            eprintln!("http: a moved request's response not moved: {e}");
        }
    }
    Ok(())
}

/// A folder or request gone, and the last responses of the requests it took.
pub fn delete_item(collection: &str, id: &str) -> Result<(), HttpError> {
    for request in change(collection, |c| c.remove(id))? {
        // Left behind, it goes when the page next opens.
        if let Err(e) = http_collections::delete_response(collection, &request) {
            eprintln!("http: a deleted request's response not deleted: {e}");
        }
    }
    Ok(())
}

/// The file at `path`, picked in the system's dialog, into the store it belongs to.
pub fn import_file(path: &Path) -> Result<ImportedAs, HttpError> {
    import(&std::fs::read_to_string(path).map_err(|e| HttpError::Store(format!("could not read {}: {e}", path.display())))?)
}

/// The stored collection as it is, at `dest`.
pub fn export_collection(id: &str, dest: &Path) -> Result<(), HttpError> {
    write_json(dest, &http_collections::get(id)?.to_text())
}

/// An environment as Postman imports it, its secrets empty.
pub fn export_environment(id: &str, dest: &Path) -> Result<(), HttpError> {
    write_json(dest, &environment_to_postman(&http_env_store::get(id)?, false))
}

/// Only to a `.json` file: `dest` comes from the window, which names it after a collection.
fn write_json(dest: &Path, text: &str) -> Result<(), HttpError> {
    if dest.extension().and_then(|e| e.to_str()) != Some("json") {
        return Err(HttpError::Store(format!("{} is not a .json file", dest.display())));
    }
    std::fs::write(dest, text).map_err(|e| HttpError::Store(format!("could not write {}: {e}", dest.display())))
}

/// The environments and globals as the window gets them, secrets masked.
#[derive(Debug, Serialize)]
pub struct Environments {
    pub environments: Vec<Environment>,
    pub globals: Vec<Variable>,
}

pub fn environments() -> Result<Environments, HttpError> {
    Ok(Environments { environments: http_env_store::list()?, globals: http_env_store::globals()?.iter().map(Variable::for_window).collect() })
}

/// One saved secret's value, for the form's eye button: the user asks for it, one
/// variable of one environment at a time. Lists and Preview keep the mask, and the
/// agent never asks here — it fills values in on this side and reads them masked.
pub fn environment_secret(id: &str, key: &str) -> Result<String, HttpError> {
    let environment = http_env_store::get(id)?;
    environment.variables.into_iter().find(|v| v.secret && v.key == key).map(|v| v.value).ok_or_else(|| HttpError::NoSecret(key.into()))
}

/// The form saved; an empty certificate is none. One that does not parse is
/// refused here, rather than at every send as a stand that does not answer.
pub fn save_environment(mut from_window: Environment) -> Result<(), HttpError> {
    from_window.trusted_cert = from_window.trusted_cert.filter(|pem| !pem.trim().is_empty());
    if let Some(pem) = &from_window.trusted_cert {
        http_agent::parse_trusted_certs(pem).map_err(|e| HttpError::BadCertificate(e.0))?;
    }
    http_env_store::save_from_window(from_window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::{AgentAccess, AgentEdits, CookieJar as _, Environment, Outcome, Variable};
    use crate::domain::postman::Collection;
    use crate::infra::http_client::tests::{closed_port, reply, serve, Seen};
    use crate::infra::http_cookies;
    use crate::testing::with_app_dir;
    use serde_json::json;
    use std::sync::mpsc::Receiver;
    use std::sync::Arc;

    const PROBE: &str = include_str!("../infra/testdata/postman/probe.postman_collection.json");

    /// One `GET {{baseUrl}}/res` under `auth`.
    fn guarded(auth: Value) -> String {
        let collection = json!({
            "info": {"name": "Guarded", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "res", "name": "Res", "request": {"method": "GET", "url": {"raw": "{{baseUrl}}/res"}, "auth": auth}}],
        });
        http_collections::add(&Collection::parse(&collection.to_string()).unwrap()).unwrap()
    }

    /// Client credentials against `{{baseUrl}}/token`, the secret being the environment's.
    fn client_credentials(more: Value) -> Value {
        let mut rows = vec![
            json!({"key": "grant_type", "value": "client_credentials"}),
            json!({"key": "accessTokenUrl", "value": "{{baseUrl}}/token"}),
            json!({"key": "clientId", "value": "app"}),
            json!({"key": "clientSecret", "value": "{{token}}"}),
        ];
        rows.extend(more.as_array().unwrap().iter().cloned());
        json!({"type": "oauth2", "oauth2": rows})
    }

    fn answer(status: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
    }

    fn got(body: &str) -> String {
        answer("200 OK", body)
    }

    fn head_of(seen: &Receiver<Seen>) -> String {
        seen.recv_timeout(std::time::Duration::from_secs(5)).unwrap().head
    }

    fn nothing_more(seen: &Receiver<Seen>) {
        assert!(seen.recv_timeout(std::time::Duration::from_millis(150)).is_err(), "a request too many");
    }

    fn status_of(view: &ResponseView) -> Option<u16> {
        match view.outcome {
            Outcome::Response { status, .. } => Some(status),
            Outcome::Failed { .. } => None,
        }
    }

    #[test]
    fn a_token_is_got_kept_and_used_again() {
        with_app_dir("http-oauth-keep", || {
            let (port, seen) = serve(vec![reply(&got(r#"{"access_token": "tok-1", "expires_in": 3600}"#)), reply(&got("{}")), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            for _ in 0..2 {
                assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            }
            let token = seen.recv().unwrap();
            assert!(token.head.starts_with("POST /token"), "{}", token.head);
            assert_eq!(token.body, b"grant_type=client_credentials");
            let client = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "app:s3cr3t-t0k");
            assert!(token.head.contains(&client), "the client, with the environment's secret: {}", token.head);
            for _ in 0..2 {
                let head = head_of(&seen);
                assert!(head.starts_with("GET /res") && head.to_ascii_lowercase().contains("authorization: bearer tok-1"), "{head}");
            }
            nothing_more(&seen);
        });
    }

    #[test]
    fn a_token_past_its_time_is_refreshed_before_the_send() {
        with_app_dir("http-oauth-expired", || {
            let (port, seen) = serve(vec![
                reply(&got(r#"{"access_token": "old", "refresh_token": "r-1", "expires_in": 0}"#)),
                reply(&got("{}")),
                reply(&got(r#"{"access_token": "new", "expires_in": 3600}"#)),
                reply(&got("{}")),
            ]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            send(&id, "res", None, Some(&env)).unwrap();
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            let (_, _, refresh, res) = (seen.recv().unwrap(), seen.recv().unwrap(), seen.recv().unwrap(), seen.recv().unwrap());
            assert_eq!(refresh.body, b"grant_type=refresh_token&refresh_token=r-1");
            assert!(res.head.to_ascii_lowercase().contains("bearer new"), "{}", res.head);
            let key = key_of(&id, &env);
            assert_eq!(http_env_store::token(&id, &key).unwrap().unwrap().refresh.as_deref(), Some("r-1"), "still good");
        });
    }

    /// The name the collection's token is kept under.
    fn key_of(id: &str, env: &str) -> String {
        let collection = http_collections::get(id).unwrap();
        let Auth::OAuth2(oauth) = collection.auth_for("res").unwrap() else { panic!("oauth2") };
        let environment = http_env_store::get(env).unwrap();
        oauth.filled(&Scope::new(&[&environment.variables[..]])).unwrap().key()
    }

    #[test]
    fn a_refused_refresh_falls_back_on_the_grant() {
        with_app_dir("http-oauth-refresh-refused", || {
            let (port, seen) = serve(vec![
                reply(&got(r#"{"access_token": "old", "refresh_token": "r-1", "expires_in": 0}"#)),
                reply(&got("{}")),
                reply(&answer("400 Bad Request", r#"{"error": "invalid_grant"}"#)),
                reply(&got(r#"{"access_token": "new"}"#)),
                reply(&got("{}")),
            ]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            send(&id, "res", None, Some(&env)).unwrap();
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            let bodies: Vec<Vec<u8>> = seen.iter().take(5).map(|s| s.body).collect();
            assert_eq!(bodies[3], b"grant_type=client_credentials");
        });
    }

    #[test]
    fn a_401_renews_the_token_once_and_not_in_a_loop() {
        with_app_dir("http-oauth-401", || {
            let refused = answer("401 Unauthorized", "{}");
            let (port, seen) = serve(vec![
                reply(&got(r#"{"access_token": "one"}"#)),
                reply(&got("{}")),
                reply(&refused),
                reply(&got(r#"{"access_token": "two"}"#)),
                reply(&refused),
                reply(&got(r#"{"access_token": "three"}"#)),
                reply(&got("{}")),
            ]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            send(&id, "res", None, Some(&env)).unwrap();
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(401), "the second refusal stands");
            let heads: Vec<String> = (0..5).map(|_| head_of(&seen)).collect();
            assert!(heads[3].starts_with("POST /token"), "{}", heads[3]);
            assert!(heads[4].to_ascii_lowercase().contains("bearer two"), "{}", heads[4]);
            nothing_more(&seen);
        });
    }

    #[test]
    fn a_token_just_got_and_refused_is_not_renewed() {
        with_app_dir("http-oauth-fresh-401", || {
            let (port, seen) = serve(vec![reply(&got(r#"{"access_token": "one"}"#)), reply(&answer("401 Unauthorized", "{}")), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(401));
            head_of(&seen);
            head_of(&seen);
            nothing_more(&seen);
        });
    }

    #[test]
    fn a_token_just_refreshed_and_refused_is_not_renewed() {
        with_app_dir("http-oauth-refreshed-401", || {
            let (port, seen) = serve(vec![
                reply(&got(r#"{"access_token": "old", "refresh_token": "r-1", "expires_in": 0}"#)),
                reply(&got("{}")),
                reply(&got(r#"{"access_token": "new"}"#)),
                reply(&answer("401 Unauthorized", "{}")),
                reply(&got(r#"{"access_token": "again"}"#)),
            ]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            send(&id, "res", None, Some(&env)).unwrap();
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(401));
            (0..4).for_each(|_| drop(head_of(&seen)));
            nothing_more(&seen);
        });
    }

    #[test]
    fn only_a_401_renews_a_token() {
        with_app_dir("http-oauth-403", || {
            let (port, seen) = serve(vec![reply(&answer("403 Forbidden", "{}")), reply(&got(r#"{"access_token": "new"}"#))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([{"key": "accessToken", "value": "from-file"}])));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(403));
            head_of(&seen);
            nothing_more(&seen);
        });
    }

    #[test]
    fn a_token_got_after_a_401_is_masked_as_the_first_was() {
        with_app_dir("http-oauth-renew-masked", || {
            let (port, _seen) = serve(vec![reply(&answer("401 Unauthorized", "{}")), reply(&got(r#"{"access_token": "tok-renewed"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([{"key": "accessToken", "value": "from-file"}, {"key": "addTokenTo", "value": "queryParams"}])));
            let view = send(&id, "res", None, Some(&env)).unwrap();
            assert_eq!(status_of(&view), Some(200));
            let text = serde_json::to_string(&http_collections::history().unwrap()).unwrap() + &serde_json::to_string(&view).unwrap();
            assert!(!text.contains("tok-renewed"), "{text}");
        });
    }

    #[test]
    fn oauth2_with_nothing_filled_in_goes_without_a_token() {
        with_app_dir("http-oauth-empty", || {
            let (port, seen) = serve_once(&got("{}"));
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(json!({"type": "oauth2", "oauth2": []}));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            assert!(!head_of(&seen).to_ascii_lowercase().contains("authorization:"));
        });
    }

    #[test]
    fn the_files_token_goes_first_and_a_401_gets_a_new_one() {
        with_app_dir("http-oauth-saved", || {
            let (port, seen) = serve(vec![reply(&answer("401 Unauthorized", "{}")), reply(&got(r#"{"access_token": "new"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([{"key": "accessToken", "value": "from-file"}])));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            assert!(head_of(&seen).to_ascii_lowercase().contains("bearer from-file"));
            assert!(head_of(&seen).starts_with("POST /token"));
            assert!(head_of(&seen).to_ascii_lowercase().contains("bearer new"));
        });
    }

    #[test]
    fn a_saved_variable_nowhere_does_not_stop_a_grant_that_can_get_a_token() {
        with_app_dir("http-oauth-unknown", || {
            let (port, seen) = serve(vec![reply(&got(r#"{"access_token": "new"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([{"key": "accessToken", "value": "{{nowhere}}"}])));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            assert!(head_of(&seen).starts_with("POST /token"));
        });
    }

    #[test]
    fn a_failed_renewal_after_a_401_leaves_the_401_and_says_why() {
        with_app_dir("http-oauth-renew-fails", || {
            let (port, _seen) = serve(vec![reply(&answer("401 Unauthorized", "{}")), reply(&answer("400 Bad Request", r#"{"error": "invalid_client"}"#))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([{"key": "accessToken", "value": "from-file"}])));
            let view = send(&id, "res", None, Some(&env)).unwrap();
            assert_eq!(status_of(&view), Some(401));
            assert_eq!(view.scripts.error.as_deref(), Some("OAuth 2.0: the token endpoint answered 400: invalid_client"));
        });
    }

    #[test]
    fn a_token_endpoint_that_refuses_stops_the_send_and_its_words_are_masked() {
        with_app_dir("http-oauth-refused", || {
            let (port, seen) = serve(vec![reply(&answer("400 Bad Request", r#"{"error": "invalid_client", "error_description": "bad secret s3cr3t-t0k"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            assert_eq!(send(&id, "res", None, Some(&env)), Err(HttpError::OAuth2("the token endpoint answered 400: invalid_client — bad secret •••".into())));
            head_of(&seen);
            nothing_more(&seen);
            assert_eq!(http_collections::history().unwrap().len(), 0);
        });
    }

    #[test]
    fn a_token_endpoint_that_is_down_is_named() {
        with_app_dir("http-oauth-down", || {
            let env = stage(format!("http://127.0.0.1:{}", closed_port()));
            let id = guarded(client_credentials(json!([])));
            let Err(HttpError::OAuth2(why)) = send(&id, "res", None, Some(&env)) else { panic!("an OAuth error") };
            assert!(why.starts_with("the token endpoint: "), "{why}");
        });
    }

    #[test]
    fn no_token_shows_in_what_is_kept_and_the_token_request_is_not_in_history() {
        with_app_dir("http-oauth-masked", || {
            let (port, _seen) = serve(vec![reply(&got(r#"{"access_token": "tok-secret", "refresh_token": "ref-secret"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([{"key": "addTokenTo", "value": "queryParams"}])));
            let view = send(&id, "res", None, Some(&env)).unwrap();
            let Outcome::Response { url, .. } = &view.outcome else { panic!("{:?}", view.outcome) };
            assert_eq!(url, &format!("http://127.0.0.1:{port}/res?access_token=•••"));
            let history = http_collections::history().unwrap();
            assert_eq!(history.len(), 1, "the request, not the token's");
            let text = serde_json::to_string(&history).unwrap() + &serde_json::to_string(&http_collections::last_response(&id, "res").unwrap()).unwrap();
            assert!(!text.contains("tok-secret") && !text.contains("ref-secret"), "{text}");
            let raw = std::fs::read(http_collections::dir().unwrap().join("environments.enc")).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("tok-secret"), "sealed");
            assert!(!serde_json::to_string(&http_collections::get(&id).unwrap().json()).unwrap().contains("tok-secret"), "the file stays as it was");
        });
    }

    #[test]
    fn a_grant_that_needs_a_browser_is_named_unless_a_token_was_saved() {
        with_app_dir("http-oauth-browser", || {
            let (port, seen) = serve_once(&got("{}"));
            let env = stage(format!("http://127.0.0.1:{port}"));
            let code = |rows: Value| json!({"type": "oauth2", "oauth2": rows});
            let grant = json!({"key": "grant_type", "value": "authorization_code"});
            let id = guarded(code(json!([grant])));
            let Err(HttpError::OAuth2(why)) = send(&id, "res", None, Some(&env)) else { panic!("an OAuth error") };
            assert!(why.contains("authorization_code"), "{why}");
            let id = guarded(code(json!([grant, {"key": "accessToken", "value": "pasted"}])));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            assert!(head_of(&seen).to_ascii_lowercase().contains("bearer pasted"));

            let id = guarded(code(json!([{"key": "grant_type", "value": "client_credentials"}])));
            assert_eq!(send(&id, "res", None, Some(&env)), Err(HttpError::OAuth2("client_credentials needs a Token URL, or a token in Access token".into())));
        });
    }

    /// `client_credentials` on a request with an `Authorization` of its own.
    fn by_hand(more: Value) -> String {
        let collection = json!({
            "info": {"name": "By hand", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "res", "name": "Res", "request": {"method": "GET", "url": {"raw": "{{baseUrl}}/res"}, "header": [{"key": "authorization", "value": "Bearer mine"}], "auth": client_credentials(more)}}],
        });
        http_collections::add(&Collection::parse(&collection.to_string()).unwrap()).unwrap()
    }

    #[test]
    fn an_authorization_written_by_hand_needs_no_token_unless_the_token_goes_in_the_query() {
        with_app_dir("http-oauth-by-hand", || {
            let (port, seen) = serve(vec![reply(&got("{}")), reply(&got(r#"{"access_token": "t"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = by_hand(json!([{"key": "accessToken", "value": "{{nowhere}}"}]));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            let head = head_of(&seen);
            assert!(head.starts_with("GET /res") && head.to_ascii_lowercase().contains("bearer mine"), "{head}");

            let id = by_hand(json!([{"key": "addTokenTo", "value": "queryParams"}]));
            send(&id, "res", None, Some(&env)).unwrap();
            assert!(head_of(&seen).starts_with("POST /token"));
            assert!(head_of(&seen).starts_with("GET /res?access_token=t"));
        });
    }

    #[test]
    fn two_sends_with_a_lapsed_token_spend_its_refresh_token_once() {
        with_app_dir("http-oauth-two-sends", || {
            let renewed = got(r#"{"access_token": "new", "expires_in": 3600}"#);
            let (port, seen) = serve(vec![Box::new(move |_| {
                std::thread::sleep(std::time::Duration::from_millis(150));
                renewed.clone().into_bytes()
            })]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            let key = key_of(&id, &env);
            http_env_store::save_token(&id, &key, Token { access: "old".into(), refresh: Some("r-1".into()), expires_at: Some(0) }).unwrap();
            let collection = http_collections::get(&id).unwrap();
            let environment = http_env_store::get(&env).unwrap();
            let scope = Scope::new(&[&environment.variables[..]]);
            let start = std::sync::Barrier::new(2);
            std::thread::scope(|threads| {
                for _ in 0..2 {
                    threads.spawn(|| {
                        start.wait();
                        assert_eq!(oauth_token(&collection, &id, "res", &scope, None).unwrap().unwrap().access, "new");
                    });
                }
            });
            head_of(&seen);
            nothing_more(&seen);
        });
    }

    #[test]
    fn a_collection_deleted_takes_its_tokens() {
        with_app_dir("http-oauth-delete", || {
            let (port, _seen) = serve(vec![reply(&got(r#"{"access_token": "one"}"#)), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(client_credentials(json!([])));
            send(&id, "res", None, Some(&env)).unwrap();
            let key = key_of(&id, &env);
            assert!(http_env_store::token(&id, &key).unwrap().is_some());
            delete_collection(&id).unwrap();
            assert_eq!(http_env_store::token(&id, &key).unwrap(), None);
        });
    }

    #[test]
    fn a_token_another_send_renewed_is_taken_not_renewed_again() {
        with_app_dir("http-oauth-race", || {
            let held = Held { oauth: OAuth2::default(), key: "k".into(), access: "stale".into(), fresh: false };
            http_env_store::save_token("c", "k", Token { access: "newer".into(), refresh: None, expires_at: None }).unwrap();
            assert_eq!(renewed(&held, "c", None).unwrap(), "newer");
        });
    }

    #[test]
    fn a_cookie_from_a_response_goes_with_the_next_send_and_the_jar_lists_it() {
        with_app_dir("http-cookies-send", || {
            let (port, seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nset-cookie: sid=v-1; Path=/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(json!({"type": "noauth"}));
            send(&id, "res", None, Some(&env)).unwrap();
            send(&id, "res", None, Some(&env)).unwrap();
            assert!(!head_of(&seen).to_ascii_lowercase().contains("cookie:"));
            assert!(head_of(&seen).to_ascii_lowercase().contains("cookie: sid=v-1"));
            assert_eq!(http_cookies::list().unwrap().iter().map(|c| (c.domain.as_str(), c.name.as_str())).collect::<Vec<_>>(), [("127.0.0.1", "sid")]);
        });
    }

    #[test]
    fn a_redirect_to_another_domain_takes_no_cookie_of_the_first() {
        with_app_dir("http-cookies-foreign-hop", || {
            let (other, elsewhere) = serve_once(&got("{}"));
            let (port, _seen) = serve_once(&format!("HTTP/1.1 302 Found\r\nset-cookie: sid=v-1; Path=/\r\nlocation: http://localhost:{other}/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"));
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(json!({"type": "noauth"}));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            assert!(!head_of(&elsewhere).to_ascii_lowercase().contains("cookie:"));
            assert_eq!(http_cookies::list().unwrap().len(), 1);
        });
    }

    #[test]
    fn a_token_request_takes_no_cookies_and_leaves_none() {
        with_app_dir("http-cookies-token", || {
            let (port, seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nset-cookie: from-token=1; Path=/\r\ncontent-length: 21\r\nconnection: close\r\n\r\n{\"access_token\": \"t\"}"), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            http_cookies_seed(port);
            let id = guarded(client_credentials(json!([])));
            send(&id, "res", None, Some(&env)).unwrap();
            assert!(!head_of(&seen).to_ascii_lowercase().contains("cookie:"), "the session stays out of the token's business");
            assert!(head_of(&seen).to_ascii_lowercase().contains("cookie: session=1"));
            assert_eq!(http_cookies::list().unwrap().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["session"]);
        });
    }

    fn http_cookies_seed(port: u16) {
        use crate::domain::http_client::CookieJar;
        StoredJar.take(&format!("http://127.0.0.1:{port}/"), &["session=1; Path=/".to_string()]);
    }

    #[test]
    fn a_scripts_request_shares_the_jar() {
        with_app_dir("http-cookies-script", || {
            let (port, seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nset-cookie: from-script=1; Path=/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let code = format!("pm.sendRequest('http://127.0.0.1:{port}/login', function () {{}});");
            let id = scripted(vec![], vec![], vec![event("prerequest", &code)]);
            trust(&id);
            send(&id, "order", None, Some(&env)).unwrap();
            head_of(&seen);
            assert!(head_of(&seen).to_ascii_lowercase().contains("cookie: from-script=1"));
        });
    }

    #[test]
    fn digest_goes_through_a_send_and_its_password_is_nowhere_kept() {
        with_app_dir("http-digest-send", || {
            let challenge = "HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Digest realm=\"probe\", nonce=\"n1\", qop=\"auth\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
            let (port, seen) = serve(vec![reply(challenge), reply(&got("{}"))]);
            let env = stage(format!("http://127.0.0.1:{port}"));
            let id = guarded(json!({"type": "digest", "digest": [{"key": "username", "value": "ann"}, {"key": "password", "value": "{{token}}"}]}));
            assert_eq!(status_of(&send(&id, "res", None, Some(&env)).unwrap()), Some(200));
            head_of(&seen);
            assert!(head_of(&seen).contains("Digest username=\"ann\""));
            assert!(!serde_json::to_string(&http_collections::history().unwrap()).unwrap().contains("s3cr3t-t0k"));
        });
    }

    #[test]
    fn formdata_reaches_the_stand_as_it_was_built() {
        with_app_dir("http-formdata-send", || {
            let (port, seen) = serve_once(&got("{}"));
            let env = stage(format!("http://127.0.0.1:{port}"));
            let mut c = Collection::new("c", "Forms");
            c.add(None, json!({"id": "f", "name": "F", "request": {"method": "POST", "url": "{{baseUrl}}/up", "body": {"mode": "formdata", "formdata": [{"key": "a", "value": "1"}]}}})).unwrap();
            let id = http_collections::add(&c).unwrap();
            send(&id, "f", None, Some(&env)).unwrap();
            let got = seen.recv().unwrap();
            let boundary = got.head.split("boundary=").nth(1).unwrap().lines().next().unwrap().trim().to_string();
            assert_eq!(String::from_utf8(got.body).unwrap(), format!("--{boundary}\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--{boundary}--\r\n"));
        });
    }

    #[test]
    fn the_eye_reads_one_saved_secret_and_nothing_that_is_not_one() {
        with_app_dir("http-secret", || {
            let env = r#"{"name": "Stage", "values": [{"key": "token", "value": "pm-hunter2", "type": "secret", "enabled": true}, {"key": "user", "value": "ann", "enabled": true}], "_postman_variable_scope": "environment"}"#;
            let ImportedAs::Environment { id, .. } = import(env).unwrap() else { panic!("an environment") };
            assert_eq!(environment_secret(&id, "token").unwrap(), "pm-hunter2");
            assert!(matches!(environment_secret(&id, "user"), Err(HttpError::NoSecret(k)) if k == "user"), "not a secret");
            assert!(matches!(environment_secret(&id, "nope"), Err(HttpError::NoSecret(_))));
            assert!(matches!(environment_secret("nope", "token"), Err(HttpError::NoEnvironment(_))));
        });
    }

    #[test]
    fn each_kind_of_file_lands_in_its_store_and_a_secret_is_sealed() {
        with_app_dir("http-import", || {
            let ImportedAs::Collection { id, name } = import(PROBE).unwrap() else { panic!("a collection") };
            assert_eq!(name, "wire_kibo_test");
            assert_eq!(http_collections::get(&id).unwrap().request_ids().len(), 10, "with the ids it was given");

            let env = r#"{"name": "Stage", "values": [{"key": "token", "value": "pm-hunter2", "type": "secret", "enabled": true}], "_postman_variable_scope": "environment"}"#;
            let ImportedAs::Environment { id, name } = import(env).unwrap() else { panic!("an environment") };
            assert_eq!(name, "Stage");
            assert_eq!(http_env_store::get(&id).unwrap().variables[0].value, "pm-hunter2");
            assert_eq!(http_env_store::list().unwrap()[0].variables[0].value, "•••");
            let raw = std::fs::read(http_collections::dir().unwrap().join("environments.enc")).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("pm-hunter2"));

            let globals = r#"{"values": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}], "_postman_variable_scope": "globals"}"#;
            assert_eq!(import(globals).unwrap(), ImportedAs::Globals { count: 2 });
            assert!(matches!(import("{\"openapi\": \"3.0.0\"}"), Err(HttpError::NotACollection(_))));
        });
    }

    #[test]
    fn a_response_whose_request_is_gone_is_pruned() {
        with_app_dir("http-prune", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let request = http_collections::get(&id).unwrap().request_ids()[0].clone();
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new(), scripts: Default::default() };
            http_collections::save_response(&id, &request, &saved).unwrap();
            http_collections::save_response(&id, "removed-request", &saved).unwrap();
            http_collections::save_response("removed-collection", "r", &saved).unwrap();
            let ImportedAs::Collection { id: broken, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            http_collections::save_response(&broken, "r", &saved).unwrap();
            let file = http_collections::dir().unwrap().join("collections").join(format!("{broken}.postman_collection.json"));
            std::fs::write(file, "{ damaged").unwrap();
            collections().unwrap();
            assert!(http_collections::last_response(&id, &request).unwrap().is_some());
            assert_eq!(http_collections::last_response(&id, "removed-request").unwrap(), None);
            assert_eq!(http_collections::last_response("removed-collection", "r").unwrap(), None);
            assert!(http_collections::last_response(&broken, "r").unwrap().is_some(), "a file that does not read keeps its responses");
        });
    }

    #[test]
    fn a_broken_file_is_listed_with_why_beside_the_ones_that_read() {
        with_app_dir("http-list", || {
            let made = new_collection("Mine").unwrap();
            let ImportedAs::Collection { id: broken, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let file = http_collections::dir().unwrap().join("collections").join(format!("{broken}.postman_collection.json"));
            std::fs::write(file, "{ damaged").unwrap();
            let listed = collections().unwrap();
            let mine = listed.iter().find(|c| c.id == made.id).unwrap();
            assert_eq!(mine.file.as_ref().unwrap().pointer("/info/name"), Some(&json!("Mine")));
            let bad = listed.iter().find(|c| c.id == broken).unwrap();
            assert!(bad.file.is_none());
            assert!(bad.error.as_deref().unwrap().contains("not JSON"), "{:?}", bad.error);
        });
    }

    #[test]
    fn the_pages_edits_change_only_what_they_name_and_the_file_reads_back_as_postman() {
        with_app_dir("http-edit", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let before = http_collections::get(&id).unwrap();
            let request = before.request_ids()[0].clone();
            save_request(&id, &request, json!({"method": "DELETE", "url": "{{baseUrl}}/x"})).unwrap();
            rename(&id, &id, "Renamed").unwrap();
            rename(&id, &request, "Gone soon").unwrap();
            add_item(&id, None, json!({"id": "fresh-1", "name": "New", "request": {"method": "GET", "url": ""}})).unwrap();
            let after = http_collections::get(&id).unwrap();
            assert_eq!(after.name(), "Renamed");
            assert_eq!(after.written_url(&request).as_deref(), Some("{{baseUrl}}/x"));
            assert_eq!(after.request_ids().last().map(String::as_str), Some("fresh-1"));
            // Every other line is the file's own.
            let (old, new) = (before.to_text(), after.to_text());
            let changed = new.lines().filter(|line| !old.lines().any(|o| o == *line)).count();
            assert!(changed < 15, "{changed} lines changed");
            assert!(matches!(add_item(&id, None, json!({"id": "FRESH-1", "name": "Twin"})), Err(HttpError::BadId(_))));
            assert!(matches!(save_request(&id, "nothing", json!({})), Err(HttpError::NoSuchItem(_))));
        });
    }

    #[test]
    fn edits_at_once_each_land() {
        with_app_dir("http-edit-together", || {
            let id = new_collection("Busy").unwrap().id;
            let ids: Vec<String> = (0..12).map(|n| format!("r{n}")).collect();
            for r in &ids {
                add_item(&id, None, json!({"id": r, "name": r, "request": {"method": "GET", "url": ""}})).unwrap();
            }
            let threads: Vec<_> = ids
                .iter()
                .map(|r| {
                    let (id, r) = (id.clone(), r.clone());
                    std::thread::spawn(move || save_request(&id, &r, json!({"method": "POST", "url": format!("/{r}")})).unwrap())
                })
                .collect();
            threads.into_iter().for_each(|t| t.join().unwrap());
            let after = http_collections::get(&id).unwrap();
            for r in &ids {
                assert_eq!(after.written_url(r), Some(format!("/{r}")), "the edit of {r} was lost");
            }
        });
    }

    #[test]
    fn a_deleted_folder_takes_its_requests_responses() {
        with_app_dir("http-delete-item", || {
            let id = new_collection("Shop").unwrap().id;
            add_item(&id, None, json!({"id": "f", "name": "F", "item": []})).unwrap();
            add_item(&id, Some("f"), json!({"id": "in", "name": "In", "request": {"url": "/a"}})).unwrap();
            add_item(&id, None, json!({"id": "out", "name": "Out", "request": {"url": "/b"}})).unwrap();
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new(), scripts: Default::default() };
            http_collections::save_response(&id, "in", &saved).unwrap();
            http_collections::save_response(&id, "out", &saved).unwrap();
            delete_item(&id, "f").unwrap();
            assert_eq!(http_collections::get(&id).unwrap().request_ids(), ["out"]);
            assert_eq!(http_collections::last_response(&id, "in").unwrap(), None);
            assert!(http_collections::last_response(&id, "out").unwrap().is_some());
        });
    }

    #[test]
    fn a_folder_moves_with_all_it_holds_and_its_responses_follow_it() {
        with_app_dir("http-move-item", || {
            let shop = new_collection("Shop").unwrap().id;
            let other = new_collection("Other").unwrap().id;
            add_item(&shop, None, json!({"id": "f", "name": "F", "item": []})).unwrap();
            add_item(&shop, Some("f"), json!({"id": "in", "name": "In", "request": {"url": "/a"}, "event": [{"listen": "test"}]})).unwrap();
            add_item(&shop, None, json!({"id": "g", "name": "G", "item": []})).unwrap();
            add_item(&shop, None, json!({"id": "out", "name": "Out", "request": {"url": "/b"}})).unwrap();
            add_item(&other, None, json!({"id": "h", "name": "H", "item": []})).unwrap();
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new(), scripts: Default::default() };
            http_collections::save_response(&shop, "in", &saved).unwrap();
            let folders = |c: &str, f: &str| http_collections::get(c).unwrap().item_mut(f).map(|i| i["item"].clone());

            // Within one collection: last into the folder named, nothing else touched.
            move_item(&shop, "out", &shop, Some("g")).unwrap();
            assert_eq!(folders(&shop, "g").unwrap(), json!([{"id": "out", "name": "Out", "request": {"url": "/b"}}]));
            move_item(&shop, "out", &shop, None).unwrap();
            assert_eq!(folders(&shop, "g").unwrap(), json!([]));
            assert_eq!(http_collections::get(&shop).unwrap().request_ids(), ["in", "out"]);

            // A folder into itself or what it holds: refused, nothing written.
            assert!(matches!(move_item(&shop, "f", &shop, Some("f")), Err(HttpError::NoSuchItem(_))));
            assert_eq!(http_collections::get(&shop).unwrap().request_ids(), ["in", "out"]);

            // Into another collection's folder: the item as it was, scripts too, and its last response.
            move_item(&shop, "f", &other, Some("h")).unwrap();
            assert_eq!(http_collections::get(&shop).unwrap().request_ids(), ["out"]);
            assert_eq!(folders(&other, "h").unwrap()[0]["item"][0]["event"], json!([{"listen": "test"}]));
            assert_eq!(http_collections::last_response(&shop, "in").unwrap(), None);
            assert!(http_collections::last_response(&other, "in").unwrap().is_some());

            // An id it holds already there: refused, and neither collection changed.
            add_item(&shop, None, json!({"id": "f2", "name": "F2", "item": [{"id": "in", "name": "Twin", "request": {"url": "/c"}}]})).unwrap();
            assert!(matches!(move_item(&shop, "f2", &other, None), Err(HttpError::BadId(id)) if id == "in"));
            assert_eq!(http_collections::get(&shop).unwrap().request_ids(), ["out", "in"]);
            assert_eq!(http_collections::get(&other).unwrap().request_ids(), ["in"]);
            assert!(matches!(move_item(&shop, "nope", &other, None), Err(HttpError::NoSuchItem(_))));
        });
    }

    #[test]
    fn an_export_imported_twice_shares_no_ids() {
        with_app_dir("http-import-twice", || {
            // With the ids Postman gave it, as an export of a collection that was imported has.
            let mut withids = Collection::parse(PROBE).unwrap();
            withids.ensure_ids();
            let text = withids.to_text();
            let ImportedAs::Collection { id: first, .. } = import(&text).unwrap() else { panic!("a collection") };
            let ImportedAs::Collection { id: second, .. } = import(&text).unwrap() else { panic!("a collection") };
            let ids = |id: &str| http_collections::get(id).unwrap().ids().into_iter().collect::<HashSet<_>>();
            let (a, b) = (ids(&first), ids(&second));
            assert_eq!(a, withids.ids().into_iter().collect(), "the first keeps its own");
            assert_eq!(b.len(), 14);
            assert!(a.is_disjoint(&b));
        });
    }

    #[test]
    fn an_export_is_the_stored_file_and_only_to_a_json_file() {
        with_app_dir("http-export", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let dir = crate::testing::temp_dir("http-export-dest");
            let dest = dir.join("shop.postman_collection.json");
            export_collection(&id, &dest).unwrap();
            assert_eq!(std::fs::read_to_string(&dest).unwrap(), http_collections::get(&id).unwrap().to_text());
            let script = dir.join("run.sh");
            assert!(export_collection(&id, &script).is_err());
            assert!(!script.exists());

            stage("http://x".into());
            let env = dir.join("stage.json");
            export_environment("stage", &env).unwrap();
            let text = std::fs::read_to_string(&env).unwrap();
            assert!(text.contains("\"_postman_variable_scope\": \"environment\""), "{text}");
            assert!(!text.contains("s3cr3t-t0k"), "{text}");
        });
    }

    #[test]
    fn a_certificate_that_does_not_parse_is_refused_and_an_empty_one_is_none() {
        use crate::infra::http_agent::tests::CA;
        with_app_dir("http-env-cert", || {
            let env = |cert: &str| Environment { id: "e".into(), name: "E".into(), variables: vec![], trusted_cert: Some(cert.into()), agent: Default::default() };
            assert!(matches!(save_environment(env("not a certificate")), Err(HttpError::BadCertificate(_))));
            assert!(matches!(http_env_store::get("e"), Err(HttpError::NoEnvironment(_))), "nothing saved");
            save_environment(env("  \n")).unwrap();
            assert_eq!(http_env_store::get("e").unwrap().trusted_cert, None);
            save_environment(env(CA)).unwrap();
            assert_eq!(http_env_store::get("e").unwrap().trusted_cert.as_deref(), Some(CA));
        });
    }

    #[test]
    fn the_window_gets_globals_with_a_secret_masked() {
        with_app_dir("http-env-list", || {
            stage("http://x".into());
            let var = |key: &str, secret| Variable { key: key.into(), value: "pm-hunter2".into(), secret, enabled: true };
            http_env_store::set_globals(vec![var("token", true), var("plain", false)]).unwrap();
            let all = environments().unwrap();
            assert_eq!(all.environments[0].variables[1].value, "•••");
            assert_eq!(all.globals.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(), ["•••", "pm-hunter2"]);
        });
    }

    /// Answers one request with `text`.
    fn serve_once(text: &str) -> (u16, Receiver<Seen>) {
        serve(vec![reply(text)])
    }

    /// What reached the stand, head and body as one text.
    fn wire(seen: &Receiver<Seen>) -> String {
        let seen = seen.recv().unwrap();
        seen.head + "\r\n" + &String::from_utf8(seen.body).unwrap()
    }

    fn shop() -> String {
        let collection = json!({
            "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}", "type": "string"}]},
            "item": [{"id": "order", "name": "Order", "request": {
                "method": "POST", "url": {"raw": "{{baseUrl}}/orders"},
                "body": {"mode": "raw", "raw": "{\"sum\": {{sum}}}", "options": {"raw": {"language": "json"}}}
            }}],
            "variable": [{"key": "sum", "value": "0"}]
        });
        http_collections::add(&Collection::parse(&collection.to_string()).unwrap()).unwrap()
    }

    fn event(listen: &str, code: &str) -> Value {
        json!({"listen": listen, "script": {"type": "text/javascript", "exec": code.lines().collect::<Vec<_>>()}})
    }

    /// A collection with scripts at each level: `events` on the collection, the
    /// folder and the request, in that order.
    fn scripted(on_collection: Vec<Value>, on_folder: Vec<Value>, on_request: Vec<Value>) -> String {
        let collection = json!({
            "info": {"name": "Scripted", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "folder", "name": "Folder", "event": on_folder, "item": [
                {"id": "order", "name": "Order", "event": on_request, "request": {"method": "POST", "url": {"raw": "{{baseUrl}}/orders"}, "body": {"mode": "raw", "raw": "{}"}}},
            ]}],
            "event": on_collection,
            "variable": [{"key": "fromFile", "value": "file"}, {"key": "apiKey", "value": "", "type": "secret"}],
        });
        http_collections::add(&Collection::parse(&collection.to_string()).unwrap()).unwrap()
    }

    /// Every script of the collection turned on.
    fn trust(id: &str) {
        let hashes: Vec<String> = script_states(id).unwrap().into_iter().flat_map(|s| s.hashes).collect();
        set_scripts(id, &hashes, true).unwrap();
    }

    fn env_value(key: &str) -> Option<String> {
        http_env_store::get("stage").unwrap().variables.into_iter().find(|v| v.key == key).map(|v| v.value)
    }

    fn global(key: &str) -> Option<String> {
        http_env_store::globals().unwrap().into_iter().find(|v| v.key == key).map(|v| v.value)
    }

    fn stage(base: String) -> String {
        let var = |key: &str, value: String, secret| Variable { key: key.into(), value, secret, enabled: true };
        let variables = vec![var("baseUrl", base, false), var("token", "s3cr3t-t0k".into(), true)];
        http_env_store::add(Environment { id: "stage".into(), name: "Stage".into(), variables, trusted_cert: None, agent: Default::default() }).unwrap();
        "stage".into()
    }

    #[test]
    fn a_send_is_on_disk_before_the_window_has_it_and_the_secret_only_on_the_wire() {
        with_app_dir("http-send", || {
            let (port, seen) = serve_once("HTTP/1.1 422 Unprocessable Entity\r\ncontent-length: 13\r\nconnection: close\r\n\r\n{\"sum\": \"<1\"}");
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}/v1"));
            // The collection's `sum` is over the globals'.
            http_env_store::set_globals(vec![Variable { key: "sum".into(), value: "9".into(), secret: false, enabled: true }]).unwrap();
            let view = send(&collection, "order", None, Some(&env)).unwrap();
            let wire = wire(&seen);
            assert!(wire.starts_with("POST /v1/orders HTTP/1.1"), "{wire}");
            assert!(wire.contains("Bearer s3cr3t-t0k"), "{wire}");
            assert!(wire.ends_with("\r\n\r\n{\"sum\": 0}"), "{wire}");

            assert!(matches!(view.outcome, Outcome::Response { status: 422, .. }), "{view:?}");
            assert_eq!((view.body.as_deref(), view.size, view.environment.as_deref()), (Some("{\"sum\": \"<1\"}"), 13, Some("Stage")));
            assert_eq!(last_response(&collection, "order").unwrap(), Some(view.clone()));

            let history = http_collections::history().unwrap();
            assert_eq!(history.len(), 1);
            let entry = &history[0];
            assert_eq!((entry.template_url.as_str(), entry.environment.as_deref(), entry.agent_chat.as_deref()), ("{{baseUrl}}/orders", Some("Stage"), None));
            assert_eq!(entry.request_id.as_deref(), Some("order"), "the entry opens its request");
            assert!(entry.request.headers.contains(&("Authorization".into(), "Bearer •••".into())), "{:?}", entry.request.headers);
            assert_eq!(entry.response_body.as_deref(), Some("{\"sum\": \"<1\"}"));
        });
    }

    #[test]
    fn the_pages_edit_is_sent_and_the_stored_request_left_as_it_was() {
        with_app_dir("http-send-edited", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}"));
            let before = http_collections::get(&collection).unwrap().to_text();
            let edited = json!({"method": "GET", "url": "{{baseUrl}}/orders/7", "auth": {"type": "noauth"}});
            send(&collection, "order", Some(edited), Some(&env)).unwrap();
            let wire = wire(&seen);
            assert!(wire.starts_with("GET /orders/7 HTTP/1.1"), "{wire}");
            assert!(!wire.to_ascii_lowercase().contains("authorization"), "{wire}");
            assert_eq!(http_collections::get(&collection).unwrap().to_text(), before);
            assert_eq!(http_collections::history().unwrap()[0].template_url, "{{baseUrl}}/orders/7");
            assert!(matches!(send(&collection, "missing", Some(json!({})), None), Err(HttpError::NoSuchItem(_))));
        });
    }

    #[test]
    fn the_environment_wins_over_the_collection_and_brings_its_certificate() {
        use crate::infra::http_agent::tests::{serve_tls_with, CA, LEAF, LEAF_KEY};
        with_app_dir("http-send-env", || {
            let port = serve_tls_with(LEAF, LEAF_KEY, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let collection = shop();
            let var = |key: &str, value: &str| Variable { key: key.into(), value: value.into(), secret: false, enabled: true };
            let variables = vec![var("baseUrl", &format!("https://127.0.0.1:{port}")), var("token", "t"), var("sum", "5")];
            http_env_store::add(Environment { id: "tls".into(), name: "TLS".into(), variables, trusted_cert: Some(CA.into()), agent: Default::default() }).unwrap();
            let view = send(&collection, "order", None, Some("tls")).unwrap();
            assert!(matches!(view.outcome, Outcome::Response { status: 200, .. }), "{view:?}");
            assert_eq!(http_collections::history().unwrap()[0].request.body.as_deref(), Some("{\"sum\": 5}"));
        });
    }

    #[test]
    fn a_stand_that_is_down_is_a_result_kept_like_a_response() {
        with_app_dir("http-send-down", || {
            let port = closed_port();
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}"));
            let view = send(&collection, "order", None, Some(&env)).unwrap();
            assert!(matches!(view.outcome, Outcome::Failed { .. }), "{view:?}");
            assert_eq!(last_response(&collection, "order").unwrap(), Some(view));
            assert_eq!(http_collections::history().unwrap().len(), 1);
        });
    }

    #[test]
    fn a_secret_in_the_address_reaches_neither_the_window_nor_the_disk() {
        with_app_dir("http-send-secret-url", || {
            let (port, seen) = serve(vec![
                reply("HTTP/1.1 302 Found\r\nlocation: /done\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
                reply("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
            ]);
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}"));
            let apikey = json!([{"key": "key", "value": "k"}, {"key": "value", "value": "{{token}}"}, {"key": "in", "value": "query"}]);
            let edited = json!({"method": "GET", "url": "{{baseUrl}}/bot{{token}}/getMe", "auth": {"type": "apikey", "apikey": apikey}});
            let view = send(&collection, "order", Some(edited), Some(&env)).unwrap();
            assert!(wire(&seen).starts_with("GET /bots3cr3t-t0k/getMe?k=s3cr3t-t0k HTTP/1.1"));
            let Outcome::Response { redirects, url, .. } = &view.outcome else { panic!("{view:?}") };
            assert_eq!((redirects[0].as_str(), url.as_str()), (format!("http://127.0.0.1:{port}/bot•••/getMe?k=•••").as_str(), format!("http://127.0.0.1:{port}/done").as_str()));

            let bad = json!({"method": "GET", "url": "http://[{{token}}/x"});
            let failed = send(&collection, "order", Some(bad), Some(&env)).unwrap();
            assert!(matches!(&failed.outcome, Outcome::Failed { error } if error.contains("http://[•••/x")), "{failed:?}");

            let shown = [
                serde_json::to_string(&view).unwrap(),
                serde_json::to_string(&failed).unwrap(),
                serde_json::to_string(&http_collections::history().unwrap()).unwrap(),
                serde_json::to_string(&last_response(&collection, "order").unwrap()).unwrap(),
            ];
            assert!(shown.iter().all(|s| !s.contains("s3cr3t")), "{shown:?}");
        });
    }

    #[test]
    fn a_send_that_ends_after_a_later_one_does_not_replace_its_response() {
        with_app_dir("http-send-race", || {
            let (slow, _seen_slow) = serve(vec![Box::new(|_| {
                std::thread::sleep(std::time::Duration::from_millis(500));
                b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nslow".to_vec()
            })]);
            let (fast, _seen_fast) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nfast");
            let collection = shop();
            let to = |port: u16| json!({"method": "GET", "url": format!("http://127.0.0.1:{port}/"), "auth": {"type": "noauth"}});
            let first = {
                let (collection, request) = (collection.clone(), to(slow));
                std::thread::spawn(move || send(&collection, "order", Some(request), None).unwrap())
            };
            std::thread::sleep(std::time::Duration::from_millis(150));
            assert_eq!(send(&collection, "order", Some(to(fast)), None).unwrap().body.as_deref(), Some("fast"));
            assert_eq!(first.join().unwrap().body.as_deref(), Some("slow"), "the window still gets it, to drop");
            assert_eq!(last_response(&collection, "order").unwrap().unwrap().body.as_deref(), Some("fast"));
            assert_eq!(http_collections::history().unwrap().len(), 2, "history logs both");
        });
    }

    #[test]
    fn a_history_that_does_not_open_does_not_cost_the_response() {
        with_app_dir("http-send-damaged", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
            let collection = shop();
            std::fs::write(http_collections::dir().unwrap().join("history.enc"), b"not sealed").unwrap();
            let edited = json!({"method": "GET", "url": format!("http://127.0.0.1:{port}/"), "auth": {"type": "noauth"}});
            let view = send(&collection, "order", Some(edited), None).unwrap();
            assert_eq!(view.body.as_deref(), Some("ok"));
            assert_eq!(last_response(&collection, "order").unwrap(), Some(view));
            assert!(http_collections::history().is_err(), "left as it was, not overwritten");
        });
    }

    #[test]
    fn a_variable_found_nowhere_stops_the_send_and_leaves_no_trace() {
        with_app_dir("http-send-missing", || {
            let collection = shop();
            assert_eq!(send(&collection, "order", None, None), Err(HttpError::UnknownVariables(vec!["baseUrl".into(), "token".into()])));
            assert_eq!(send(&collection, "order", None, Some("gone")), Err(HttpError::NoEnvironment("gone".into())));
            assert_eq!(last_response(&collection, "order").unwrap(), None);
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn a_script_that_changes_a_header_changes_what_is_sent() {
        with_app_dir("http-script-header", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.request.headers.add({key: 'X-Trace', value: pm.environment.get('baseUrl').length + ''});\npm.request.url.addQueryParams('from=script');\npm.request.body.update('{\"changed\":1}');")]);
            trust(&id);
            let base = format!("http://127.0.0.1:{port}");
            let length = base.len();
            stage(base);
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.error, None);
            assert!(matches!(view.outcome, Outcome::Response { .. }), "{:?}", view.outcome);
            let wire = wire(&seen);
            assert!(wire.contains(&format!("x-trace: {length}")) || wire.contains(&format!("X-Trace: {length}")), "{wire}");
            assert!(wire.starts_with("POST /orders?from=script "), "{wire}");
            assert!(wire.ends_with("{\"changed\":1}"), "{wire}");
        });
    }

    #[test]
    fn scripts_run_collection_folder_request_and_each_sees_the_last_ones_writes() {
        with_app_dir("http-script-order", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            let step = |name: &str| format!("pm.globals.set('order', (pm.globals.get('order') || '') + '{name}');");
            let id = scripted(
                vec![event("prerequest", &step("c")), event("test", &step("C"))],
                vec![event("prerequest", &step("f")), event("test", &step("F"))],
                vec![event("prerequest", &step("r")), event("test", &step("R"))],
            );
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(global("order").as_deref(), Some("cfrCFR"));
        });
    }

    /// Imported, every script is off and the send says which did not run. Turned on one
    /// by one — a folder's on runs for all its requests — and by the hash of its code: a
    /// script changed is off again, its old hash refused, the others stay on.
    #[test]
    fn a_script_runs_only_once_turned_on_and_off_again_once_changed() {
        with_app_dir("http-script-on", || {
            let (port, _seen) = serve((0..4).map(|_| reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")).collect());
            let id = scripted(vec![], vec![event("prerequest", "pm.globals.set('folder', 'ran');")], vec![event("prerequest", "pm.globals.set('ran', 'yes');")]);
            stage(format!("http://127.0.0.1:{port}"));
            let states = script_states(&id).unwrap();
            assert_eq!(states.iter().map(|s| (s.item.as_deref(), s.on)).collect::<Vec<_>>(), [(Some("folder"), false), (Some("order"), false)]);

            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.off, ["pre-request of folder \"Folder\"", "pre-request of request \"Order\""]);
            assert_eq!((global("folder"), global("ran")), (None, None), "the request went without them");

            set_scripts(&id, &states[0].hashes, true).unwrap();
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap().scripts.off, ["pre-request of request \"Order\""]);
            assert_eq!((global("folder").as_deref(), global("ran")), (Some("ran"), None));

            set_scripts(&id, &states[1].hashes, true).unwrap();
            assert!(send(&id, "order", None, Some("stage")).unwrap().scripts.off.is_empty());
            assert_eq!(global("ran").as_deref(), Some("yes"));

            change(&id, |c| {
                c.item_mut("order").unwrap().insert("event".into(), json!([event("prerequest", "pm.globals.set('ran', 'changed');")]));
                Ok(())
            })
            .unwrap();
            assert_eq!(script_states(&id).unwrap().iter().map(|s| s.on).collect::<Vec<_>>(), [true, false], "another script is off; the folder's stays on");
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap().scripts.off, ["pre-request of request \"Order\""]);
            assert_eq!(global("ran").as_deref(), Some("yes"));
            assert!(set_scripts(&id, &states[1].hashes, true).is_err(), "an answer about the old script is no answer about this one");
            set_scripts(&id, &states[0].hashes, false).unwrap();
            assert!(!script_states(&id).unwrap()[0].on, "turned off again");
        });
    }

    /// The same code is on wherever an import puts it: what was turned on stays on
    /// after the collection comes in again, and what changed in it is off.
    #[test]
    fn a_collection_imported_again_keeps_on_what_did_not_change() {
        with_app_dir("http-script-reimport", || {
            let id = scripted(vec![event("prerequest", "login()")], vec![], vec![event("test", "check()")]);
            trust(&id);
            change(&id, |c| {
                c.item_mut("order").unwrap().insert("event".into(), json!([event("test", "check(); steal()")]));
                c.item_mut("folder").unwrap().insert("id".into(), json!("renumbered"));
                Ok(())
            })
            .unwrap();
            assert_eq!(script_states(&id).unwrap().iter().map(|s| (s.item.as_deref(), s.on)).collect::<Vec<_>>(), [(None, true), (Some("order"), false)]);
        });
    }

    /// Trust given to a whole collection before HC-11a: its scripts unchanged since are
    /// all on, once; changed since, none. The old entry is gone either way.
    #[test]
    fn trust_in_a_whole_collection_carries_over_once() {
        with_app_dir("http-script-carry", || {
            let id = scripted(vec![event("prerequest", "a()")], vec![], vec![event("test", "b()")]);
            let digest = http_collections::get(&id).unwrap().scripts_digest().unwrap().hash;
            settings_store::update(|s| s.http_script_trust.insert(id.clone(), digest)).unwrap();
            assert!(script_states(&id).unwrap().iter().all(|s| s.on));
            assert!(!settings_store::load().unwrap().http_script_trust.contains_key(&id));

            let other = scripted(vec![event("prerequest", "a()")], vec![], vec![]);
            settings_store::update(|s| s.http_script_trust.insert(other.clone(), "stale".into())).unwrap();
            assert!(script_states(&other).unwrap().iter().all(|s| !s.on), "changed since: none");
            assert!(!settings_store::load().unwrap().http_script_trust.contains_key(&other));
        });
    }

    /// Two scripts of a kind on one level are one switch: on only when both are — a
    /// re-import that changed one of them shows the level off.
    #[test]
    fn a_level_is_on_only_when_all_its_scripts_are() {
        with_app_dir("http-script-half", || {
            let id = scripted(vec![event("prerequest", "a()"), event("prerequest", "b()")], vec![], vec![]);
            let hashes = script_states(&id).unwrap().remove(0).hashes;
            assert_eq!(hashes.len(), 2);
            set_scripts(&id, &hashes[..1], true).unwrap();
            assert!(!script_states(&id).unwrap()[0].on);
            set_scripts(&id, &hashes, true).unwrap();
            assert!(script_states(&id).unwrap()[0].on);
        });
    }

    /// A switch drops the hashes of scripts the collection no longer has: the set in the
    /// settings does not grow with every import.
    #[test]
    fn a_switch_forgets_the_scripts_gone() {
        with_app_dir("http-script-gone", || {
            let id = scripted(vec![event("prerequest", "a()")], vec![], vec![event("test", "b()")]);
            trust(&id);
            change(&id, |c| {
                c.item_mut("order").unwrap().insert("event".into(), json!([]));
                Ok(())
            })
            .unwrap();
            set_scripts(&id, &[script_hash("a()")], false).unwrap();
            assert_eq!(settings_store::load().unwrap().http_scripts_on.get(&id), Some(&BTreeSet::new()));
        });
    }

    /// Nothing in the collection's file turns a script on: a flag an author or an
    /// exporter put there arrives with the import, so it is not read (decision 2).
    #[test]
    fn nothing_in_the_file_turns_a_script_on() {
        with_app_dir("http-script-flag", || {
            let mut flagged = event("prerequest", "a()");
            flagged["disabled"] = json!(false);
            flagged["on"] = json!(true);
            flagged["trusted"] = json!(true);
            flagged["script"]["hash"] = json!(script_hash("a()"));
            let (port, _seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
            let id = scripted(vec![flagged], vec![], vec![]);
            stage(format!("http://127.0.0.1:{port}"));
            assert_eq!(script_states(&id).unwrap().iter().map(|s| s.on).collect::<Vec<_>>(), [false]);
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap().scripts.off, ["pre-request of collection \"Scripted\""]);
        });
    }

    /// What the Scripts tab shows is the code the send runs and the switch turns on: a
    /// line of `exec` that is not a string is dropped from both, never shown alone.
    #[test]
    fn the_code_shown_is_the_code_hashed_and_run() {
        with_app_dir("http-script-shown", || {
            let odd = json!({"listen": "prerequest", "script": {"exec": ["const note = `", ["`/*"], "`; steal(); //", "/**/", ""]}});
            let id = scripted(vec![odd], vec![], vec![]);
            let state = script_states(&id).unwrap().remove(0);
            assert_eq!(state.codes, ["const note = `\n`; steal(); //\n/**/\n"]);
            assert_eq!(state.hashes, [script_hash(&state.codes[0])]);
        });
    }

    /// Decision 5: the user's edit of a script on keeps it on, of one off keeps it off, and
    /// a script written where there was none is on; blank code takes it away.
    #[test]
    fn an_edit_keeps_a_script_as_it_was_and_a_new_one_is_on() {
        with_app_dir("http-script-edit", || {
            let id = scripted(vec![event("prerequest", "collection()")], vec![], vec![event("prerequest", "mine()")]);
            let on = |item: Option<&str>, listen: &str| script_states(&id).unwrap().into_iter().find(|s| s.item.as_deref() == item && s.listen == listen).map(|s| (s.codes, s.on));
            set_scripts(&id, &[script_hash("mine()")], true).unwrap();

            save_script(&id, "order", "prerequest", "mine(); more()").unwrap();
            assert_eq!(on(Some("order"), "prerequest"), Some((vec!["mine(); more()".to_string()], true)), "on, edited: on");
            set_scripts(&id, &[script_hash("mine(); more()")], false).unwrap();
            save_script(&id, "order", "prerequest", "mine(); again()").unwrap();
            assert_eq!(on(Some("order"), "prerequest"), Some((vec!["mine(); again()".to_string()], false)), "off, edited: off");

            save_script(&id, "order", "test", "check()").unwrap();
            assert_eq!(on(Some("order"), "test"), Some((vec!["check()".to_string()], false)), "new: off until switched on");
            assert_eq!(on(None, "prerequest").map(|(_, on)| on), Some(false), "the collection's untouched");

            save_script(&id, "order", "test", " ").unwrap();
            assert_eq!(on(Some("order"), "test"), None, "blank: gone");
            assert!(!settings_store::load().unwrap().http_scripts_on[&id].contains(&script_hash(" ")), "blank is not turned on");
            assert!(save_script(&id, "order", "prerequest-hack", "x()").is_err());
            assert!(save_script(&id, "folder", "test", "x()").is_err(), "a folder's are not written from a request's tab");
        });
    }

    /// A script off cleared, then put back as it was — undo after a pause, cut and paste —
    /// is off still: the write in between left nothing, and "nothing there" turns on nothing.
    #[test]
    fn a_script_off_cleared_and_put_back_stays_off() {
        with_app_dir("http-script-undo", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "foreign()")]);
            save_script(&id, "order", "prerequest", "").unwrap();
            save_script(&id, "order", "prerequest", "foreign()").unwrap();
            assert!(!script_states(&id).unwrap()[0].on);
        });
    }

    /// Two of a kind, one on and one off: the edit that writes over both is off.
    #[test]
    fn an_edit_over_a_script_on_and_one_off_is_off() {
        with_app_dir("http-script-mixed", || {
            let id = scripted(vec![], vec![], vec![event("test", "mine()"), event("test", "theirs()")]);
            set_scripts(&id, &[script_hash("mine()")], true).unwrap();
            save_script(&id, "order", "test", "mine(); theirs()").unwrap();
            assert!(!script_states(&id).unwrap()[0].on);
        });
    }

    /// Each pause writes the code as it was; the hashes of those gone are dropped, so a
    /// script off that comes to read like one of them is not on by that.
    #[test]
    fn the_code_of_each_pause_is_forgotten() {
        with_app_dir("http-script-pauses", || {
            let id = scripted(vec![], vec![], vec![event("test", "a()")]);
            set_scripts(&id, &[script_hash("a()")], true).unwrap();
            save_script(&id, "order", "test", "a(); b").unwrap();
            save_script(&id, "order", "test", "a(); b()").unwrap();
            let set = settings_store::load().unwrap().http_scripts_on[&id].clone();
            assert_eq!(set, BTreeSet::from([script_hash("a(); b()")]));
        });
    }

    /// Trust given to the whole collection before HC-11a counts as on for an edit too.
    #[test]
    fn an_edit_of_a_script_trusted_before_stays_on() {
        with_app_dir("http-script-edit-carry", || {
            let id = scripted(vec![], vec![], vec![event("test", "old()")]);
            let digest = http_collections::get(&id).unwrap().scripts_digest().unwrap().hash;
            settings_store::update(|s| s.http_script_trust.insert(id.clone(), digest)).unwrap();
            save_script(&id, "order", "test", "new()").unwrap();
            assert!(script_states(&id).unwrap()[0].on);
        });
    }

    #[test]
    fn a_collection_with_no_scripts_has_nothing_to_turn_on() {
        with_app_dir("http-script-none", || {
            let id = shop();
            assert_eq!(script_states(&id).unwrap(), []);
            assert!(set_scripts(&id, &["anything".to_string()], true).is_err());
            assert!(set_scripts(&id, &[], true).is_err());
        });
    }

    #[test]
    fn what_scripts_write_outlives_the_send_and_stays_out_of_the_collections_file() {
        with_app_dir("http-script-vars", || {
            let (port, _seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
            let code = "pm.globals.set('access_token', 't-1');\npm.environment.set('seen', pm.collectionVariables.get('fromFile'));\npm.collectionVariables.set('fromFile', 'script');\npm.collectionVariables.set('token', 'c-1');\npm.variables.set('local', 'gone');\npm.environment.unset('baseUrl2');";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            let file = http_collections::get(&id).unwrap().json().clone();
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();

            assert!(view.scripts.changed);
            assert_eq!(global("access_token").as_deref(), Some("t-1"));
            assert_eq!(env_value("seen").as_deref(), Some("file"));
            assert_eq!(env_value("local"), None);
            assert_eq!(http_collections::get(&id).unwrap().json(), &file, "the file is as it came");
            let listed = collections().unwrap().into_iter().find(|c| c.id == id).unwrap();
            assert_eq!(listed.current.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect::<Vec<_>>(), vec![("fromFile", "script"), ("token", "c-1")]);

            // The next send reads what this one wrote.
            change(&id, |c| {
                c.item_mut("order").unwrap().insert("event".into(), json!([event("prerequest", "pm.environment.set('again', pm.collectionVariables.get('token') + pm.globals.get('access_token'));")]));
                Ok(())
            })
            .unwrap();
            trust(&id);
            send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(env_value("again").as_deref(), Some("c-1t-1"));

            settings_store::update(|s| s.http_script_trust.insert(id.clone(), "old".into())).unwrap();
            delete_collection(&id).unwrap();
            assert!(!http_env_store::collection_values().unwrap().contains_key(&id));
            let settings = settings_store::load().unwrap();
            assert!(!settings.http_scripts_on.contains_key(&id));
            assert!(!settings.http_script_trust.contains_key(&id));
        });
    }

    #[test]
    fn a_script_that_writes_nothing_says_so() {
        with_app_dir("http-script-quiet", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.environment.set('baseUrl', pm.environment.get('baseUrl'));")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            assert!(!send(&id, "order", None, Some("stage")).unwrap().scripts.changed);
        });
    }

    #[test]
    fn send_request_goes_out_and_into_history_with_a_secret_masked() {
        with_app_dir("http-script-sub", || {
            let token = "HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{\"access_token\":\"x\"}";
            let (port, seen) = serve(vec![reply(token), reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
            let code = "pm.request.headers.add({key: 'X-Own', value: pm.environment.get('token')});\npm.sendRequest({url: pm.environment.get('baseUrl') + '/token?k=' + pm.environment.get('token'), method: 'POST', header: {'X-Key': pm.environment.get('token')}, body: {mode: 'urlencoded', urlencoded: [{key: 'grant_type', value: 'client_credentials'}]}}, (err, res) => { pm.globals.set('access_token', res.json().access_token); });";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();

            let first = wire(&seen);
            assert!(first.starts_with("POST /token?k=s3cr3t-t0k "), "{first}");
            assert!(first.ends_with("grant_type=client_credentials"), "{first}");
            assert!(wire(&seen).starts_with("POST /orders "));
            assert_eq!(global("access_token").as_deref(), Some("x"));

            let history = http_collections::history().unwrap();
            assert_eq!(history.len(), 2);
            let sub = history.iter().find(|h| h.request_id.is_none()).expect("the script's own request is in history");
            assert!(sub.request.url.ends_with("/token?k=•••"), "{}", sub.request.url);
            assert!(!serde_json::to_string(&history).unwrap().contains("s3cr3t-t0k"), "{history:?}");
        });
    }

    #[test]
    fn a_pre_request_script_that_fails_sends_nothing_and_keeps_what_it_wrote() {
        with_app_dir("http-script-throw", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.globals.set('step', '1');\nthrow new Error('no ' + pm.environment.get('token'));")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            let error = send(&id, "order", None, Some("stage")).unwrap_err().to_string();
            assert!(error.contains("nothing was sent"), "{error}");
            assert!(error.contains("Error: no •••"), "{error}");
            assert_eq!(global("step").as_deref(), Some("1"));
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn skipping_the_request_sends_nothing() {
        with_app_dir("http-script-skip", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.execution.skipRequest();")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            assert!(send(&id, "order", None, Some("stage")).unwrap_err().to_string().contains("skipped"));
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn tests_are_kept_with_the_response_and_a_test_that_throws_leaves_it_standing() {
        with_app_dir("http-script-tests", || {
            let (port, _seen) = serve_once("HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 8\r\n\r\n{\"id\":7}");
            let first = "pm.test('created', () => pm.response.to.have.status(201));\npm.test('id is ' + pm.response.json().id, () => pm.expect(pm.response.json().id).to.eql(8));\nconsole.log(pm.environment.get('token'), pm.request.url.toString());\nthrow new Error('after the tests');";
            let second = "pm.globals.set('second', 'ran');";
            let id = scripted(vec![], vec![event("test", first)], vec![event("test", second)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(matches!(view.outcome, Outcome::Response { status: 201, .. }));
            assert_eq!(view.scripts.tests.iter().map(|t| (t.name.as_str(), t.passed)).collect::<Vec<_>>(), vec![("created", true), ("id is 7", false)]);
            assert_eq!(view.scripts.error.as_deref(), Some("Error: after the tests"));
            assert_eq!(global("second").as_deref(), Some("ran"), "the next script still runs");
            assert!(view.scripts.console[0].text.starts_with("••• http://127.0.0.1"), "{:?}", view.scripts.console);
            assert_eq!(last_response(&id, "order").unwrap().unwrap().scripts.tests.len(), 2, "the saved response has them too");
        });
    }

    #[test]
    fn with_no_response_there_is_nothing_to_test() {
        with_app_dir("http-script-down", || {
            let id = scripted(vec![], vec![], vec![event("test", "pm.globals.set('tested', 'yes');")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(matches!(view.outcome, Outcome::Failed { .. }));
            assert_eq!(global("tested"), None);
            assert_eq!(view.scripts.error, None);
        });
    }

    #[test]
    fn a_script_that_changes_nothing_changes_nothing_on_the_wire() {
        with_app_dir("http-script-same-wire", || {
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let (port, seen) = serve(vec![reply(answer), reply(answer)]);
            let file = |events: Value| {
                let request = json!({"method": "POST", "url": {"raw": format!("http://u:p@127.0.0.1:{port}/x?a=1&b#h")}, "header": [{"key": "X-On", "value": "1"}, {"key": "X-Off", "value": "0", "disabled": true}],
                    "body": {"mode": "raw", "raw": "{\"a\":1}", "options": {"raw": {"language": "json"}}}});
                let file = json!({"info": {"name": "C", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"}, "item": [{"id": "order", "name": "Order", "event": events, "request": request}]});
                http_collections::add(&Collection::parse(&file.to_string()).unwrap()).unwrap()
            };
            let (plain, scripted) = (file(json!([])), file(json!([event("prerequest", "1;")])));
            trust(&scripted);
            send(&plain, "order", None, None).unwrap();
            send(&scripted, "order", None, None).unwrap();
            assert_eq!(wire(&seen), wire(&seen));
        });
    }

    #[test]
    fn a_later_send_wins_though_the_first_is_still_in_its_scripts() {
        with_app_dir("http-script-race", || {
            let reply_with = |text: &str| reply(&format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len()));
            let (port, _seen) = serve(vec![reply_with("second"), reply_with("first")]);
            let id = scripted(vec![], vec![], vec![event("prerequest", "setTimeout(() => {}, pm.globals.has('quick') ? 0 : 600);")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let first = {
                let id = id.clone();
                std::thread::spawn(move || send(&id, "order", None, Some("stage")).unwrap())
            };
            std::thread::sleep(std::time::Duration::from_millis(300));
            http_env_store::write_changes(None, &[("quick".into(), Some("1".into()))], ("", &[])).unwrap();
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap().body.as_deref(), Some("second"));
            assert_eq!(first.join().unwrap().body.as_deref(), Some("first"));
            assert_eq!(last_response(&id, "order").unwrap().unwrap().body.as_deref(), Some("second"));
        });
    }

    #[test]
    fn a_secret_a_script_replaced_or_covered_is_still_masked_everywhere() {
        with_app_dir("http-script-old-secret", || {
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let (port, _seen) = serve(vec![reply(answer), reply(answer)]);
            let code = "const old = pm.environment.get('token');\npm.globals.set('copy', old);\npm.request.headers.add({key: 'X-Old', value: old});\npm.environment.set('token', 'n3w-t0k3n');\npm.variables.set('token', 'plain');";
            let next = "pm.sendRequest(pm.environment.get('baseUrl') + '/' + pm.globals.get('copy'), () => {});";
            let id = scripted(vec![], vec![event("prerequest", code)], vec![event("prerequest", next)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            let history = serde_json::to_string(&http_collections::history().unwrap()).unwrap();
            assert!(!history.contains("s3cr3t-t0k"), "{history}");
        });
        with_app_dir("http-script-old-secret-error", || {
            let code = "const old = pm.environment.get('token');\npm.environment.set('token', 'new');\nthrow new Error(old);";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            let error = send(&id, "order", None, Some("stage")).unwrap_err().to_string();
            assert!(!error.contains("s3cr3t-t0k") && error.contains("•••"), "{error}");
        });
    }

    #[test]
    fn what_scripts_wrote_and_could_not_keep_is_said() {
        with_app_dir("http-script-unkept", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("test", "setTimeout(() => pm.globals.set('late', '1'), 400);")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let sent = std::thread::spawn(move || send(&id, "order", None, Some("stage")).unwrap());
            seen.recv().unwrap();
            let store = http_collections::dir().unwrap().join("environments.enc");
            std::fs::write(&store, b"not sealed").unwrap();
            let view = sent.join().unwrap();
            assert!(view.scripts.error.as_deref().is_some_and(|e| e.contains("not kept")), "{:?}", view.scripts);
            assert!(!view.scripts.changed);
        });
    }

    #[test]
    fn a_loop_without_end_in_a_pre_request_script_sends_nothing() {
        with_app_dir("http-script-loop", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "while (true) {}")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap_err().to_string(), "a script ran out of time");
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn a_test_script_past_the_limit_stops_the_rest_and_the_response_stands() {
        with_app_dir("http-script-test-loop", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![event("test", "while (true) {}")], vec![event("test", "pm.globals.set('after', 'ran');")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(matches!(view.outcome, Outcome::Response { status: 200, .. }));
            assert_eq!(view.scripts.error.as_deref(), Some("a script ran out of time"));
            assert_eq!(global("after"), None);
        });
    }

    #[test]
    fn what_a_test_script_makes_a_secret_is_masked_in_the_address_it_came_from() {
        with_app_dir("http-script-learned", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("test", "pm.environment.set('token', 'orders');")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(env_value("token").as_deref(), Some("orders"));
            assert!(!serde_json::to_string(&view.outcome).unwrap().contains("orders"), "{:?}", view.outcome);
        });
    }

    #[test]
    fn a_collections_secret_a_script_wrote_reaches_the_window_masked() {
        with_app_dir("http-script-current", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.collectionVariables.set('apiKey', 's3cr3t-k3y');\npm.collectionVariables.set('open', 'plain');")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            let all = collections().unwrap();
            let current = &all.iter().find(|c| c.id == id).unwrap().current;
            assert!(current.iter().any(|v| v.key == "apiKey" && v.secret && v.value != "s3cr3t-k3y"), "{current:?}");
            assert!(current.iter().any(|v| v.key == "open" && v.value == "plain"), "{current:?}");
        });
    }

    #[test]
    fn a_script_s_request_that_fails_reaches_its_callback_as_an_error() {
        with_app_dir("http-script-sub-down", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let code = format!("pm.sendRequest('http://127.0.0.1:{}/', (err, res) => pm.globals.set('said', err ? 'failed' : 'answered'));", closed_port());
            let id = scripted(vec![], vec![], vec![event("prerequest", &code)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(global("said").as_deref(), Some("failed"));
        });
    }

    #[test]
    fn a_test_script_sees_the_request_as_it_left() {
        with_app_dir("http-script-sent", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("test", "pm.test('body', () => pm.expect(pm.request.body.raw).to.eql('{}'));")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.tests.iter().map(|t| t.passed).collect::<Vec<_>>(), vec![true], "{:?}", view.scripts.tests);
        });
    }

    #[test]
    fn a_secret_a_script_replaces_stays_hidden_old_and_new() {
        with_app_dir("http-script-secret", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let code = "console.log(pm.environment.get('token'));\npm.environment.set('token', 'n3w-t0k3n');\nconsole.log(pm.environment.get('token'));";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.console.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["•••", "•••"]);
            let stored = http_env_store::get("stage").unwrap();
            assert!(stored.variables.iter().any(|v| v.key == "token" && v.secret && v.value == "n3w-t0k3n"), "still a secret, with its new value");
        });
    }

    /// Counts the page's signals, and says the chat's the one in `title`.
    fn stands(chat: &str) -> (Stands, Arc<AtomicU64>) {
        let told = Arc::new(AtomicU64::new(0));
        let counted = told.clone();
        (Stands { changed: Arc::new(move |_| { counted.fetch_add(1, Ordering::Relaxed); }), chat: chat.into() }, told)
    }

    /// Stage, open to the agent, and a collection whose one request goes to `{{baseUrl}}`.
    fn opened_stage(base: String) {
        let var = |key: &str, value: String, secret| Variable { key: key.into(), value, secret, enabled: true };
        let variables = vec![var("baseUrl", base, false), var("token", "s3cr3t-t0k".into(), true)];
        http_env_store::add(Environment { id: "stage".into(), name: "Stage".into(), variables, trusted_cert: None, agent: AgentAccess::ReadOnly }).unwrap();
        shop_to("{{baseUrl}}/orders");
    }

    /// A collection Shop with one request, `list`, to `url`; its auth a bearer `{{apiKey}}`.
    fn shop_to(url: &str) -> String {
        let shop = json!({
            "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{apiKey}}"}]},
            "item": [{"id": "list", "name": "List", "request": {"method": "GET", "url": url}}]
        });
        http_collections::add(&Collection::parse(&shop.to_string()).unwrap()).unwrap()
    }

    /// Stage at `base`, open to the agent as `access` says, its `token` a secret.
    fn open_stage(base: String, access: AgentAccess) {
        let var = |key: &str, value: String, secret| Variable { key: key.into(), value, secret, enabled: true };
        let variables = vec![var("baseUrl", base, false), var("token", "s3cr3t-t0k".into(), true)];
        http_env_store::add(Environment { id: "stage".into(), name: "Stage".into(), variables, trusted_cert: None, agent: access }).unwrap();
    }

    /// `open_stage`, and Shop under a bearer of `{{token}}`: `list` reads
    /// `{{baseUrl}}/orders?id=1`, `make` posts `{"sum": 1}` to `{{baseUrl}}/orders`.
    fn agent_stage(base: String, access: AgentAccess) -> String {
        open_stage(base, access);
        let shop = json!({
            "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
            "item": [
                {"id": "list", "name": "List", "request": {"method": "GET", "url": "{{baseUrl}}/orders?id=1"}},
                {"id": "make", "name": "Make", "request": {"method": "POST", "url": "{{baseUrl}}/orders", "body": {"mode": "raw", "raw": "{\"sum\": 1}", "options": {"raw": {"language": "json"}}}}},
            ]
        });
        http_collections::add(&Collection::parse(&shop.to_string()).unwrap()).unwrap()
    }

    fn agent(request: &str) -> AgentSend {
        AgentSend { request: request.into(), ..AgentSend::default() }
    }

    fn agent_with(request: &str, edits: AgentEdits) -> AgentSend {
        AgentSend { request: request.into(), edits, variables: vec![] }
    }

    fn body_of(sent: &AgentSent) -> String {
        String::from_utf8_lossy(&sent.reply.body).into_owned()
    }

    #[test]
    fn the_agent_sees_only_the_environments_opened_to_it() {
        with_app_dir("http-stands-open", || {
            stage("http://127.0.0.1:1".into());
            let (stands, _) = stands("c");
            assert_eq!(stands.open().unwrap(), []);
            http_env_store::delete("stage").unwrap();
            opened_stage("http://127.0.0.1:1".into());
            assert_eq!(stands.open().unwrap(), [OpenStand { id: "stage".into(), name: "Stage".into(), access: AgentAccess::ReadOnly }]);
        });
    }

    /// The page's Send of the saved request, the model's edits and values over it: the
    /// secret on the wire only, History signed by the chat with what the model read, and
    /// neither the request's last response nor the collection touched.
    #[test]
    fn the_agents_send_goes_as_the_pages_with_its_edits_and_lands_in_history_as_read() {
        with_app_dir("http-stands-send", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\nset-cookie: sid=1\r\nx-echo: s3cr3t-t0k\r\ncontent-length: 21\r\nconnection: close\r\n\r\n{\"you\":\"s3cr3t-t0k\"}\r\n");
            let shop = agent_stage(format!("http://127.0.0.1:{port}/v1"), AgentAccess::ReadOnly);
            crate::infra::chat_store::save_plain("c1", crate::domain::chat_role::ChatRole::Api, None, &[crate::domain::llm::LlmMessage::user("check the stand")], &json!([])).unwrap();
            let (stands, told) = stands("c1");
            let edits = AgentEdits { path: Some("/orders/{{n}}".into()), query: Some("expand=items".into()), headers: vec![("X-Case".into(), Some("{{n}}".into()))], ..AgentEdits::default() };
            let sent = stands.send("stage", &AgentSend { variables: vec![("n".into(), "2".into())], ..agent_with("list", edits) }).unwrap();

            let wire = wire(&seen);
            assert!(wire.starts_with("GET /v1/orders/2?expand=items HTTP/1.1"), "{wire}");
            assert!(wire.contains("Bearer s3cr3t-t0k") && wire.to_ascii_lowercase().contains("x-case: 2"), "{wire}");
            assert_eq!((sent.method.as_str(), sent.url.as_str()), ("GET", "{{baseUrl}}/orders/{{n}}?expand=items"));
            let text = format!("{:?} {}", sent.reply.outcome, body_of(&sent));
            assert!(!text.contains("s3cr3t-t0k") && !text.to_ascii_lowercase().contains("set-cookie"), "{text}");
            assert!(body_of(&sent).contains("\"you\": \"•••\""), "{text}");

            let history = http_collections::history().unwrap();
            let entry = &history[0];
            assert_eq!((entry.template_url.as_str(), entry.environment.as_deref(), entry.agent_chat.as_deref(), entry.request_id.as_deref()), ("{{baseUrl}}/orders/{{n}}?expand=items", Some("Stage"), Some("check the stand"), Some("list")));
            assert_eq!(entry.outcome, sent.reply.outcome);
            assert_eq!(entry.response_body.as_deref().map(str::as_bytes), Some(&sent.reply.body[..]));
            assert!(entry.request.headers.contains(&("Authorization".into(), "Bearer •••".into())), "{:?}", entry.request.headers);
            assert!(!serde_json::to_string(entry).unwrap().contains("s3cr3t-t0k"));
            assert_eq!(told.load(Ordering::Relaxed), 1, "the page is told");
            assert_eq!(last_response(&shop, "list").unwrap(), None, "the request's last response is the user's");
            assert_eq!(http_collections::get(&shop).unwrap().written_url("list").as_deref(), Some("{{baseUrl}}/orders?id=1"), "the edits are not saved");
        });
    }

    /// A write goes with the model's body, typed as JSON; history keeps it masked;
    /// the card is told the method and the request it goes from.
    #[test]
    fn a_write_carries_the_models_body_and_history_keeps_it_masked() {
        with_app_dir("http-stands-write", || {
            let (port, seen) = serve_once("HTTP/1.1 201 Created\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            agent_stage(format!("http://127.0.0.1:{port}"), AgentAccess::AskForWrites);
            let (stands, _) = stands("c");
            assert_eq!(stands.open().unwrap()[0].access, AgentAccess::AskForWrites, "the mode reaches the agent");
            let send = agent_with("make", AgentEdits { body: Some(r#"{"key": "{{token}}"}"#.into()), ..AgentEdits::default() });
            assert_eq!(stands.sending("stage", &send).unwrap(), ("POST".to_string(), "POST {{baseUrl}}/orders, from \"Shop / Make\"".to_string()));
            stands.send("stage", &send).unwrap();

            let wire = wire(&seen).to_ascii_lowercase();
            assert!(wire.starts_with("post /orders http/1.1"), "{wire}");
            assert!(wire.contains("content-type: application/json"), "{wire}");
            assert!(wire.ends_with(r#"{"key": "s3cr3t-t0k"}"#), "{wire}");
            let entry = &http_collections::history().unwrap()[0];
            assert_eq!(entry.request.body.as_deref(), Some(r#"{"key": "•••"}"#));
            assert!(!serde_json::to_string(entry).unwrap().contains("s3cr3t-t0k"));
        });
    }

    /// `auth: "none"` is no auth at all: not the collection's `oauth2` — no token is
    /// asked for — and not a request's own bearer.
    #[test]
    fn no_auth_drops_what_is_inherited_and_what_is_its_own() {
        with_app_dir("http-stands-no-auth", || {
            let ok = "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
            let (port, seen) = serve(vec![reply(ok), reply(ok)]);
            open_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let signed = json!({
                "info": {"name": "Signed", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
                "auth": {"type": "oauth2", "oauth2": [{"key": "accessTokenUrl", "value": "{{baseUrl}}/token"}, {"key": "grant_type", "value": "client_credentials"}, {"key": "clientId", "value": "c"}, {"key": "clientSecret", "value": "{{token}}"}]},
                "item": [
                    {"id": "me", "name": "Me", "request": {"method": "GET", "url": "{{baseUrl}}/me"}},
                    {"id": "own", "name": "Own", "request": {"method": "GET", "url": "{{baseUrl}}/own", "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]}}},
                ]
            });
            http_collections::add(&Collection::parse(&signed.to_string()).unwrap()).unwrap();
            let (stands, _) = stands("c");
            for (request, line) in [("me", "GET /me "), ("own", "GET /own ")] {
                stands.send("stage", &agent_with(request, AgentEdits { no_auth: true, ..AgentEdits::default() })).unwrap();
                let wire = wire(&seen);
                assert!(wire.starts_with(line), "no token was asked for: {wire}");
                assert!(!wire.to_ascii_lowercase().contains("authorization"), "{wire}");
            }
        });
    }

    /// A trusted pre-request script signs in by a POST, in a read only environment too:
    /// what it got is kept for the next send, and the model never reads it.
    #[test]
    fn a_token_a_trusted_script_got_serves_the_next_send_unread() {
        with_app_dir("http-stands-login", || {
            let echo = "HTTP/1.1 200 OK\r\ncontent-length: 18\r\nconnection: close\r\n\r\nyou are s3ss-v4lue";
            let login = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 25\r\nconnection: close\r\n\r\n{\"session\": \"s3ss-v4lue\"}";
            let (port, seen) = serve(vec![reply(login), reply(echo), reply(echo)]);
            open_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let code = "if (!pm.environment.get('session')) {\n  pm.sendRequest({url: pm.environment.get('baseUrl') + '/login', method: 'POST'}, (err, res) => pm.environment.set('session', res.json().session));\n}";
            let app = json!({
                "info": {"name": "App", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
                "event": [event("prerequest", code)],
                "item": [{"id": "me", "name": "Me", "request": {"method": "GET", "url": "{{baseUrl}}/me", "header": [{"key": "X-Session", "value": "{{session}}"}]}}]
            });
            let id = http_collections::add(&Collection::parse(&app.to_string()).unwrap()).unwrap();
            trust(&id);
            let (stands, _) = stands("c");
            let first = stands.send("stage", &agent("me")).unwrap();
            let second = stands.send("stage", &agent("me")).unwrap();
            let wires: Vec<String> = (0..3).map(|_| wire(&seen)).collect();
            assert!(wires[0].starts_with("POST /login "), "{wires:?}");
            assert!(wires[1].starts_with("GET /me ") && wires[2].starts_with("GET /me "), "signed in once: {wires:?}");
            assert!(wires[1..].iter().all(|w| w.to_ascii_lowercase().contains("x-session: s3ss-v4lue")), "{wires:?}");
            assert_eq!(env_value("session").as_deref(), Some("s3ss-v4lue"), "kept, as after a Send");
            for sent in [&first, &second] {
                assert_eq!(body_of(sent), "you are •••");
                assert!(sent.scripts.off.is_empty());
            }
            // The script's own sign-in is the user's, as after a Send; the model's sends are what it read.
            let history = http_collections::history().unwrap();
            let signed: Vec<_> = history.iter().filter(|e| e.agent_chat.is_some()).collect();
            assert_eq!(signed.len(), 2);
            assert!(!serde_json::to_string(&signed).unwrap().contains("s3ss-v4lue"), "{signed:?}");
        });
    }

    /// A credential the collection writes out — no variable marks it — is masked in what
    /// comes back too. The template is found in its own collection, not the first one.
    #[test]
    fn a_credential_written_out_is_masked_in_the_reply() {
        with_app_dir("http-stands-literal", || {
            let body = r#"{"key": "l1teral-k3y"}"#;
            let (port, _seen) = serve_once(&format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()));
            open_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let collection = |name: &str, item: Value| {
                let root = json!({"info": {"name": name, "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"}, "item": [item]});
                http_collections::add(&Collection::parse(&root.to_string()).unwrap()).unwrap()
            };
            collection("Other", json!({"id": "x", "name": "X", "request": "{{baseUrl}}/x"}));
            collection("Keyed", json!({"id": "k", "name": "K", "request": {"method": "GET", "url": "{{baseUrl}}/k?api_key=l1teral-k3y", "header": [{"key": "X-Api-Key", "value": "l1teral-k3y"}]}}));
            collection("Last", json!({"id": "z", "name": "Z", "request": "{{baseUrl}}/z"}));
            let sent = stands("c").0.send("stage", &agent("k")).unwrap();
            assert_eq!(body_of(&sent), "{\n  \"key\": \"•••\"\n}");
            assert_eq!(sent.url, "{{baseUrl}}/k?api_key=•••");

            // A case kept from it: in its own collection of three, the query not told.
            let kept = stands("c").0.save("stage", &AgentSave { request: "k".into(), name: "Kept".into(), ..AgentSave::default() }).unwrap();
            assert_eq!((kept.place.as_str(), kept.url.as_str()), ("Keyed", "{{baseUrl}}/k"));
            let mut counts: Vec<(String, usize)> = http_collections::all().unwrap().into_iter().map(|(_, c)| c.unwrap()).map(|c| (c.name(), c.request_ids().len())).collect();
            counts.sort();
            assert_eq!(counts, [("Keyed".to_string(), 2), ("Last".to_string(), 1), ("Other".to_string(), 1)]);
        });
    }

    /// A variable with a plain name that another request writes where a credential
    /// goes is masked in what this one gets back.
    #[test]
    fn a_credential_by_where_it_is_written_is_masked_in_the_reply() {
        with_app_dir("http-stands-placed", || {
            let body = "echo sig-v4lue-9";
            let (port, _seen) = serve_once(&format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()));
            open_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let root = json!({
                "info": {"name": "Signed", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
                "variable": [{"key": "sig", "value": "sig-v4lue-9"}],
                "item": [
                    {"id": "k", "name": "K", "request": "{{baseUrl}}/k"},
                    {"id": "m", "name": "M", "request": {"method": "POST", "url": "{{baseUrl}}/m", "header": [{"key": "X-Api-Key", "value": "{{sig}}"}]}},
                ]
            });
            http_collections::add(&Collection::parse(&root.to_string()).unwrap()).unwrap();
            let sent = stands("c").0.send("stage", &agent("k")).unwrap();
            assert_eq!(body_of(&sent), "echo •••");
        });
    }

    /// A case is added to the template's collection — beside it, in a folder by name or a
    /// new one — and nothing else there changes: the requests, their order, the scripts'
    /// trust. The page is told. Refused as a send is, and nothing written.
    #[test]
    fn a_case_is_added_and_nothing_else_changes() {
        with_app_dir("http-stands-save", || {
            agent_stage("http://127.0.0.1:1".into(), AgentAccess::ReadOnly);
            let id = http_collections::all().unwrap().into_iter().map(|(id, _)| id).next().unwrap();
            change(&id, |c| {
                c.add(None, json!({"id": "cases", "name": "Cases", "item": []}))?;
                c.set_request("list", json!({"method": "GET", "url": "{{baseUrl}}/orders?id=1"}))?;
                Ok(())
            })
            .unwrap();
            let mut scripted = http_collections::get(&id).unwrap();
            scripted.item_mut("list").unwrap().insert("event".into(), json!([event("test", "pm.test('ok', () => {});")]));
            http_collections::save(&id, &scripted).unwrap();
            trust(&id);
            let before = http_collections::get(&id).unwrap();
            let (stands, told) = stands("c");
            let save = |name: &str, folder: Option<&str>, edits: AgentEdits| AgentSave { request: "list".into(), name: name.into(), folder: folder.map(String::from), edits };

            let beside = stands.save("stage", &save("No id", None, AgentEdits { query: Some("".into()), ..AgentEdits::default() })).unwrap();
            assert_eq!((beside.place.as_str(), beside.method.as_str(), beside.url.as_str()), ("Shop", "GET", "{{baseUrl}}/orders"));
            let filed = stands.save("stage", &save("Negative", Some("cases"), AgentEdits { method: Some("POST".into()), body: Some("{}".into()), ..AgentEdits::default() })).unwrap();
            assert_eq!(filed.place, "Shop / Cases");
            let made = stands.save("stage", &save("Again", Some("Smoke"), AgentEdits::default())).unwrap();
            assert_eq!(made.place, "Shop / Smoke");
            assert_eq!(told.load(Ordering::Relaxed), 3, "the page reads again");

            let after = http_collections::get(&id).unwrap();
            let names = |c: &Collection| c.json()["item"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
            assert_eq!(names(&after), ["List", "Make", "Cases", "No id", "Smoke"], "added last, the rest in its order");
            for at in 0..2 {
                assert_eq!(before.json()["item"][at], after.json()["item"][at], "the template and its neighbour untouched");
            }
            let mut after = after;
            assert_eq!(after.item_mut(&filed.id).unwrap()["request"]["body"], json!({"mode": "raw", "raw": "{}", "options": {"raw": {"language": "json"}}}));
            assert!(after.item_mut(&beside.id).unwrap().get("event").is_none(), "no scripts in a case");
            assert_eq!(after.json()["item"][4]["item"][0]["id"], made.id.as_str());
            assert!(script_states(&id).unwrap().iter().all(|s| s.on), "a case has no scripts: what was on stays on");

            let away = save("Away", None, AgentEdits { path: Some("@evil.example/x".into()), ..AgentEdits::default() });
            assert_eq!(stands.save("stage", &away), Err(HttpError::BadPath("@evil.example/x".into())));
            assert_eq!(http_collections::get(&id).unwrap().request_ids().len(), 5, "nothing written");

            // A template in a folder: the folder named is made in its folder, not at the top.
            change(&id, |c| c.add(Some("cases"), json!({"id": "deep", "name": "Deep", "request": {"method": "GET", "url": "{{baseUrl}}/deep"}}))).unwrap();
            let inside = stands.save("stage", &AgentSave { request: "deep".into(), name: "Deep again".into(), folder: Some("Neg".into()), edits: AgentEdits::default() }).unwrap();
            assert_eq!(inside.place, "Shop / Cases / Neg");
            let kept = http_collections::get(&id).unwrap();
            let cases = &kept.json()["item"][2]["item"];
            assert_eq!((&cases[2]["name"], &cases[2]["item"][0]["id"]), (&json!("Neg"), &json!(inside.id)));
        });
    }

    #[test]
    fn scripts_not_trusted_do_not_run_and_it_is_said() {
        with_app_dir("http-stands-untrusted", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            open_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            scripted(vec![], vec![], vec![event("prerequest", "pm.globals.set('ran', 'yes');")]);
            let sent = stands("c").0.send("stage", &agent_with("order", AgentEdits { method: Some("GET".into()), ..AgentEdits::default() })).unwrap();
            assert_eq!(sent.scripts.off, ["pre-request of request \"Order\""]);
            assert_eq!(global("ran"), None);
        });
    }

    #[test]
    fn a_redirect_is_the_models_to_read() {
        with_app_dir("http-stands-redirect", || {
            let moved = "HTTP/1.1 302 Found\r\nlocation: /elsewhere\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
            let (port, seen) = serve(vec![reply(moved), reply("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
            agent_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let sent = stands("c").0.send("stage", &agent("list")).unwrap();
            assert!(matches!(sent.reply.outcome, Outcome::Response { status: 302, .. }), "{:?}", sent.reply.outcome);
            wire(&seen);
            assert!(seen.recv_timeout(std::time::Duration::from_millis(100)).is_err(), "not followed");
        });
    }

    #[test]
    fn a_chat_gone_still_signs_its_send() {
        with_app_dir("http-stands-no-title", || {
            let (port, _seen) = serve_once("HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n");
            agent_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            stands("c9").0.send("stage", &agent("list")).unwrap();
            let entry = &http_collections::history().unwrap()[0];
            assert_eq!((entry.agent_chat.as_deref(), entry.template_url.as_str()), (Some("c9"), "{{baseUrl}}/orders?id=1"));
        });
    }

    /// As the page's Send: the user's session goes, and what the stand sets stays in it.
    #[test]
    fn the_agents_send_leaves_the_pages_session_alone() {
        with_app_dir("http-stands-jar", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\nset-cookie: sid=theirs\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let base = format!("http://127.0.0.1:{port}");
            StoredJar.take(&format!("{base}/"), &["session=users".to_string()]);
            agent_stage(base, AgentAccess::ReadOnly);
            stands("c").0.send("stage", &agent("list")).unwrap();
            assert!(!wire(&seen).to_ascii_lowercase().contains("cookie:"), "the user's session is not the agent's to send or to echo");
            let names: Vec<String> = crate::infra::http_cookies::list().unwrap().into_iter().map(|c| c.name).collect();
            assert_eq!(names, ["session"], "nor to change");
        });
    }

    /// A wrong token the model sends itself comes back to it as the stand said it; a
    /// credential the collection writes out does not.
    #[test]
    fn what_the_model_wrote_itself_it_reads_back() {
        with_app_dir("http-stands-own", || {
            let body = "token wrong-tok-1 invalid";
            let (port, _seen) = serve_once(&format!("HTTP/1.1 401 Unauthorized\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()));
            agent_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let edits = AgentEdits { headers: vec![("Authorization".into(), Some("Bearer wrong-tok-1".into()))], no_auth: true, ..AgentEdits::default() };
            let sent = stands("c").0.send("stage", &agent_with("list", edits)).unwrap();
            assert_eq!(body_of(&sent), body);
        });
    }

    #[test]
    fn a_stand_on_its_own_certificate_answers_the_agent_as_it_does_the_page() {
        use crate::infra::http_agent::tests::{serve_tls_with, CA, LEAF, LEAF_KEY};
        with_app_dir("http-stands-tls", || {
            let port = serve_tls_with(LEAF, LEAF_KEY, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            agent_stage(format!("https://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let mut environment = http_env_store::get("stage").unwrap();
            environment.trusted_cert = Some(CA.into());
            http_env_store::delete("stage").unwrap();
            http_env_store::add(environment).unwrap();
            let sent = stands("c").0.send("stage", &agent("list")).unwrap();
            assert!(matches!(sent.reply.outcome, Outcome::Response { status: 200, .. }), "{:?}", sent.reply.outcome);
        });
    }

    #[test]
    fn what_is_refused_reaches_neither_the_network_nor_history() {
        with_app_dir("http-stands-refused", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            agent_stage(format!("http://127.0.0.1:{port}"), AgentAccess::ReadOnly);
            let (stands, told) = stands("c");
            assert!(matches!(stands.send("stage", &agent("make")), Err(HttpError::ReadOnly { .. })));
            let away = agent_with("list", AgentEdits { path: Some("//evil.test/x".into()), ..AgentEdits::default() });
            assert!(matches!(stands.send("stage", &away), Err(HttpError::BadPath(_))));
            let moved = AgentSend { variables: vec![("baseUrl".into(), "https://evil.test".into())], ..agent("list") };
            assert!(matches!(stands.send("stage", &moved), Err(HttpError::VariableRefused { .. })));
            assert!(seen.recv_timeout(std::time::Duration::from_millis(100)).is_err());
            assert_eq!(http_collections::history().unwrap(), []);
            assert_eq!(told.load(Ordering::Relaxed), 0);
        });
    }

    #[test]
    fn access_taken_back_mid_turn_counts_from_the_next_call() {
        with_app_dir("http-stands-closed", || {
            agent_stage("http://127.0.0.1:1".into(), AgentAccess::ReadOnly);
            let (stands, _) = stands("c");
            let open = http_env_store::get("stage").unwrap();
            http_env_store::save_from_window(Environment { agent: AgentAccess::Off, ..open.for_window() }).unwrap();
            assert_eq!(stands.send("stage", &agent("list")), Err(HttpError::NoneOpen));
            assert_eq!(stands.sending("stage", &agent("list")), Err(HttpError::NoneOpen));
        });
    }

    #[test]
    fn a_request_deleted_mid_turn_is_gone() {
        with_app_dir("http-stands-deleted", || {
            let shop = agent_stage("http://127.0.0.1:1".into(), AgentAccess::ReadOnly);
            let (stands, _) = stands("c");
            http_collections::delete(&shop).unwrap();
            assert_eq!(stands.send("stage", &agent("list")), Err(HttpError::NoRequest("list".into())));
        });
    }

    /// What a collection's scripts wrote lies over its file, as on the page.
    #[test]
    fn a_collections_address_is_what_its_scripts_last_wrote() {
        with_app_dir("http-stands-written", || {
            opened_stage("http://127.0.0.1:1".into());
            let shop = json!({
                "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
                "variable": [{"key": "host", "value": "https://file.test"}],
                "item": [{"id": "r", "name": "r", "request": "{{host}}/x"}]
            });
            let id = http_collections::add(&Collection::parse(&shop.to_string()).unwrap()).unwrap();
            http_env_store::write_changes(None, &[], (&id, &[("host".into(), Some("https://script.test".into()))])).unwrap();
            assert_eq!(stands("c").0.describe("stage").unwrap().origins, ["http://127.0.0.1:1", "https://script.test"]);
        });
    }

    /// The stored collection, read through the store's environment, the collection's auth named.
    #[test]
    fn the_agent_reads_a_stored_request_and_nothing_is_sent() {
        with_app_dir("http-stands-read", || {
            opened_stage("http://127.0.0.1:1".into());
            let (stands, told) = stands("c");
            let text = stands.read("stage", "list").unwrap();
            assert!(text.starts_with("Request \"Shop / List\", id list: GET {{baseUrl}}/orders"), "{text}");
            assert!(text.contains("Auth: bearer, inherited from the collection\n  token = {{apiKey}}"), "{text}");
            assert_eq!(stands.read("stage", "nope"), Err(HttpError::NoRequest("nope".into())));
            assert_eq!(stands.read("gone", "list"), Err(HttpError::NoEnvironment("gone".into())));
            assert_eq!(told.load(Ordering::Relaxed), 0, "history gained nothing");
            assert!(http_collections::history().unwrap().is_empty(), "nothing sent");
        });
    }

    /// The collections, their variables over the file's, the globals and the
    /// environment, as a send from the page fills an address in.
    #[test]
    fn the_description_is_of_the_stored_collections_as_the_page_fills_them() {
        with_app_dir("http-stands-describe", || {
            opened_stage("http://127.0.0.1:1".into());
            let mut environment = http_env_store::get("stage").unwrap();
            environment.variables.push(Variable { key: "apiKey".into(), value: "k3y".into(), secret: false, enabled: true });
            http_env_store::save_from_window(environment.for_window()).unwrap();
            http_env_store::set_globals(vec![Variable { key: "g".into(), value: "https://global.test".into(), secret: false, enabled: true }]).unwrap();
            shop_to("{{g}}/x");
            let view = stands("c").0.describe("stage").unwrap();
            assert_eq!(view.origins, ["http://127.0.0.1:1", "https://global.test"]);
            assert_eq!(view.variables, [("baseUrl".to_string(), Some("http://127.0.0.1:1".to_string())), ("token".to_string(), None), ("apiKey".to_string(), None)]);
            let mut requests = view.requests.clone();
            requests.sort();
            assert_eq!(requests, ["Shop / List: GET https://global.test/x (id list)", "Shop / List: GET {{baseUrl}}/orders (id list)"], "a global written out, the environment's name left");
            assert_eq!(stands("c").0.describe("nope"), Err(HttpError::NoEnvironment("nope".into())));
        });
    }

    /// The live test's stand (HC-9) is `infra::http_client::tests::live_stand`; here, what it is opened with.
    mod live {
        use super::*;
        pub(super) use crate::infra::http_client::tests::live_stand::{stand, Hit, TOKEN};

        /// Stage, open to the agent read only, with the token secret; a collection
        /// whose one request reads order 42 with it.
        pub(super) fn environment(port: u16) {
            let var = |key: &str, value: String, secret| Variable { key: key.into(), value, secret, enabled: true };
            let variables = vec![var("baseUrl", format!("http://127.0.0.1:{port}"), false), var("token", TOKEN.into(), true)];
            http_env_store::add(Environment { id: "stage".into(), name: "Stage".into(), variables, trusted_cert: None, agent: AgentAccess::ReadOnly }).unwrap();
            let orders = json!({
                "info": {"name": "Orders", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
                "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}", "type": "string"}]},
                "item": [{"id": "get", "name": "Get order", "request": {"method": "GET", "url": "{{baseUrl}}/orders/42"}}]
            });
            http_collections::add(&Collection::parse(&orders.to_string()).unwrap()).unwrap();
        }
    }

    /// The stand checks itself with no model: otherwise the live test could "find" a bug
    /// that is not there, or miss one for a stand that refuses everything.
    #[test]
    fn http_live_stand_has_its_bug_and_nothing_else() {
        with_app_dir("http-live-stand", || {
            let (port, got) = live::stand();
            live::environment(port);
            let (stands, _) = stands("c");
            let status = |path: &str, query: Option<&str>, no_auth: bool| {
                let edits = AgentEdits { path: Some(path.into()), query: query.map(Into::into), no_auth, ..AgentEdits::default() };
                let sent = stands.send("stage", &agent_with("get", edits)).unwrap();
                assert!(!body_of(&sent).contains(live::TOKEN), "the echo is masked: {}", body_of(&sent));
                match sent.reply.outcome {
                    Outcome::Response { status, .. } => status,
                    Outcome::Failed { error } => panic!("{error}"),
                }
            };
            assert_eq!(status("/orders/42", None, false), 200);
            assert_eq!(status("/orders/999", Some("expand=items"), false), 200, "the planted bug: the spec says 404");
            assert_eq!(status("/nope", None, false), 404);
            assert_eq!(status("/orders/42", None, true), 401);
            let got = got.lock().unwrap();
            assert_eq!(got.iter().map(|h| (h.authorized, h.planted)).collect::<Vec<_>>(), [(true, false), (true, true), (true, false), (false, false)]);
        });
    }

    /// The API role's turn on the stand with `session`'s model, and what the stand got.
    fn api_role_on_the_stand(session: &crate::services::llm_session::LlmSession) -> (crate::domain::turn::ChatStreamOutcome, Vec<live::Hit>) {
        use crate::domain::chat_role::ChatRole;
        use crate::domain::features::Features;
        use crate::domain::kube::KubeSetup;
        use crate::domain::llm::LlmMessage;
        use crate::domain::tools::ApprovalPolicy;
        use crate::domain::turn::ChatEventSink;
        use crate::services::plain_chat::{self, ChatTurn};
        use std::time::{Duration, Instant};

        let (port, got) = live::stand();
        live::environment(port);
        let http: Arc<dyn crate::domain::http_client::HttpStands> = Arc::new(stands("live").0);
        let events: ChatEventSink = Arc::new(|_| {});
        let started = Instant::now();
        let cancelled = || started.elapsed() > Duration::from_secs(300);
        let approval = ApprovalPolicy::default();
        let chat = ChatTurn {
            session,
            role: ChatRole::Api,
            kube: &KubeSetup::NotSet,
            cluster: None,
            changes: None,
            runbooks: &[],
            web: None,
            scope: None,
            approval: &approval,
            events: &events,
            cancelled: &cancelled,
            log_call: &|_| {},
            board: None,
            mongo: None,
            http: Some(http),
            deepseek: None,
            knowledge: None,
            // As the user has it: the HTTP client on, and nothing else of the Catalog.
            features: Features::from_settings(&[("http-client".to_string(), true)].into()),
        };
        let prompt = "There is an orders API on my HTTP environment Stage. Its spec: `GET /orders/{id}` answers 200 \
            with the order, and 404 when there is no order with that id. Check whether the API follows the spec for \
            an id that does not exist. Answer with one line that starts with PASS or FAIL, then why.";
        let outcome = plain_chat::start(&chat, vec![LlmMessage::user(prompt)]).expect("the turn runs");
        let got = got.lock().unwrap().clone();
        (outcome, got)
    }

    /// The hidden check: the turn ends with an answer, and the answer says FAIL and not
    /// PASS; the stand answered with its planted bug to a request with the right token —
    /// the stand says so, whatever the path looked like; no write reached it; the token's
    /// value is nowhere in what the model read; History signs each send with the chat.
    fn found_the_bug(outcome: &crate::domain::turn::ChatStreamOutcome, got: &[live::Hit]) -> Result<(), String> {
        let crate::domain::turn::ChatStreamOutcome::Done(done) = outcome else { return Err(format!("the turn did not end with an answer: {outcome:?}")) };
        if let Some(rounds) = done.limit_reached {
            return Err(format!("the turn ran out of rounds after {rounds}"));
        }
        let answer = done.result.text.trim();
        let words: Vec<&str> = answer.split(|c: char| !c.is_alphanumeric()).collect();
        if !words.contains(&"FAIL") || words.contains(&"PASS") {
            return Err(format!("the bug is not named: {answer}"));
        }
        if !got.iter().any(|hit| hit.authorized && hit.planted) {
            return Err(format!("no read of an order that does not exist, with the token: {got:?}"));
        }
        if let Some(write) = got.iter().find(|hit| !matches!(hit.method.as_str(), "GET" | "HEAD" | "OPTIONS")) {
            return Err(format!("a write reached the stand: {} {}", write.method, write.path));
        }
        if serde_json::to_string(&done.history).unwrap().contains(live::TOKEN) {
            return Err("the model read the token".into());
        }
        let history = http_collections::history().unwrap();
        if history.is_empty() || history.iter().any(|e| e.agent_chat.is_none()) {
            return Err(format!("History does not sign the agent's sends: {history:?}"));
        }
        Ok(())
    }

    /// The live test's turn and check, with a model from a script: the way a model should
    /// go passes, and each way it could go wrong does not — so a pass of the live test
    /// means the model found the bug, not that the check is blind.
    #[test]
    fn http_live_check_passes_the_right_turn_only() {
        use crate::domain::llm::{ChatStreamResult, LlmToolCall};
        let said = |text: &str| ChatStreamResult { text: text.into(), ..Default::default() };
        let send = |arguments: Value| ChatStreamResult { tool_calls: vec![LlmToolCall { id: "c1".into(), name: "httpSend".into(), arguments: arguments.to_string() }], ..said("") };
        let get = |path: &str, auth: Option<&str>| send(json!({"request": "get", "path": path, "headers": {"Authorization": auth}}));
        let missing = || get("/orders/999", None);
        let head = send(json!({"request": "get", "method": "HEAD", "path": "/orders/7"}));
        let check = |label: &str, replies: Vec<ChatStreamResult>| {
            with_app_dir(label, || {
                let (outcome, got) = api_role_on_the_stand(&crate::testing::scripted_session(replies));
                found_the_bug(&outcome, &got)
            })
        };
        assert_eq!(check("http-live-right", vec![missing(), said("**FAIL** — /orders/999 answers 200, the spec says 404")]), Ok(()));
        assert_eq!(check("http-live-verdict", vec![missing(), said("I checked order 999.\nVerdict: FAIL — 200, not 404")]), Ok(()));
        assert_eq!(check("http-live-head", vec![head, said("FAIL: a HEAD for order 7 answers 200")]), Ok(()));
        let wrong = [
            ("http-live-pass", vec![missing(), said("PASS — it answers")]),
            ("http-live-no-read", vec![said("FAIL, surely")]),
            ("http-live-only-42", vec![get("/orders/42", None), said("FAIL")]),
            ("http-live-42-query", vec![send(json!({"request": "get", "query": "expand=items"})), said("FAIL")]),
            ("http-live-both", vec![missing(), said("PASS or FAIL: FAIL")]),
            ("http-live-no-token", vec![get("/orders/999", Some("Bearer nope")), said("FAIL")]),
            ("http-live-no-auth", vec![send(json!({"request": "get", "path": "/orders/999", "auth": "none"})), said("FAIL")]),
        ];
        for (label, replies) in wrong {
            assert!(check(label, replies).is_err(), "{label}");
        }
    }

    /// A real model in the API role finds the planted bug through `httpSend`.
    /// The provider is the agent bench's (`AGENT_BENCH_*`, see the top of `agent_bench.rs`):
    ///
    /// ```text
    /// AGENT_BENCH_API_KEY=… AGENT_BENCH_MODEL=… cargo test http_live -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a real model; see the doc comment"]
    fn http_live_the_api_role_finds_the_planted_bug() {
        let session = crate::services::agent_bench::session();
        with_app_dir("http-live", || {
            let (outcome, got) = api_role_on_the_stand(&session);
            if let crate::domain::turn::ChatStreamOutcome::Done(done) = &outcome {
                println!("answer: {}", done.result.text.trim());
            }
            println!("the stand got: {got:?}");
            found_the_bug(&outcome, &got).unwrap();
        });
    }
}
