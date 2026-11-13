//! `httpDraftScript`'s run (`docs/45-http-client.md`, HC-11 decision 7, HC-11c): the
//! model's script in the sandbox with nothing sent — a post-response on a send of this
//! chat's as the model read it, a pre-request on the saved request — over the variables
//! as the model sees them, a hidden one as `•••`. Nothing it writes is kept, and the
//! draft goes to its own file: the collection is not touched, so nothing runs it.

use serde_json::Value;

use super::http_client::{one_request, with_collections};
use crate::domain::http_client::{credential_literals, DraftPlan, HttpError, Outcome, Outgoing, Prepared, Scope, Variable, SECRET_MASK};
use crate::domain::http_draft::{draft_writes, fingerprint, hidden_named, masked_places, masked_strings, request_changes, AgentDraft, DraftReport, DraftSend, ScriptDraft, NOT_SENT};
use crate::domain::http_scripts::{request_after, request_for_script, request_sent, response_for, ScriptVars};
use crate::domain::postman::Collection;
use crate::infra::postman_sandbox::{Sandbox, LIMITS};
use crate::infra::{http_collections, http_env_store};

/// Runs `draft` for the chat `chat` into the environment `environment_id`, and keeps it.
pub(super) fn run(chat: &str, chat_title: &str, environment_id: &str, draft: &AgentDraft) -> Result<ScriptDraft, HttpError> {
    let listen = draft.listen.trim();
    if !matches!(listen, "prerequest" | "test") {
        return Err(HttpError::BadListen(listen.into()));
    }
    if let Some(exchange) = draft.exchange.as_deref().map(str::trim).filter(|e| !e.is_empty() && listen == "prerequest") {
        return Err(HttpError::BadExchange { exchange: exchange.into(), why: "a pre-request draft runs on the saved request, not on a send — leave `exchange` out" });
    }
    let environment = http_env_store::get(environment_id)?;
    let request_id = draft.request.trim();
    let (collection_id, plan) = with_collections(|collections, globals, ids| environment.agent_draft(collections, globals, request_id).map(|plan| (ids[plan.collection].to_string(), plan)))??;
    let mut collection = http_collections::get(&collection_id)?;
    let [globals, collection_vars, environment_vars] = plan.layers.clone();
    let local = draft.variables.iter().map(|(key, value)| Variable { key: key.clone(), value: value.clone(), secret: false, enabled: true }).collect();
    let start = ScriptVars { globals, collection: collection_vars, environment: environment_vars, local };
    let mut report = DraftReport { supplied: draft.variables.iter().map(|(k, _)| k.clone()).collect(), hidden: hidden_named(&start, &draft.code), ..DraftReport::default() };
    // A credential written into the collection by hand is masked as the model reads the
    // request: the script gets the request with it masked, so nothing it reports — however
    // it turns the value — holds it.
    let literals = collection.written_credentials(request_id)?;
    let mask = || start.scope().keeping(literals.iter().map(String::as_str));
    let mut saved = collection.item_mut(request_id).and_then(|item| item.get("request").cloned()).ok_or_else(|| HttpError::NoRequest(request_id.into()))?;
    masked_strings(&mut saved, &mask());

    // What the script is given: a post-response the send it runs on, found before the sandbox boots.
    let context = if listen == "test" {
        let history = http_collections::history()?;
        let entry = fingerprint(&history, chat, request_id, draft.exchange.as_deref())?;
        let Outcome::Response { status, headers, elapsed_ms, .. } = &entry.outcome else {
            return Err(HttpError::BadExchange { exchange: entry.id.clone(), why: "it got no response; send again" });
        };
        // The send as History keeps it, and the collection's written credentials masked over
        // it too: a send kept before they were masked on sending has them as written.
        let sent = Prepared { sent: entry.request.clone(), shown: entry.request.clone() }.hidden(&mask()).shown;
        let body = mask().hide(entry.response_body.as_deref().unwrap_or_default());
        report.exchange = Some(entry.id.clone());
        report.ran_on = format!("{} {} → {status}", sent.method, sent.url);
        report.masked = masked_places(&sent, &entry.outcome, Some(&body));
        start.context(&request_sent(&sent), Some(&response_for(*status, headers, body.as_bytes(), *elapsed_ms)))
    } else {
        start.context(&request_for_script(&saved), None)
    };

    let name = collection.item_name(request_id).unwrap_or_default();
    let scope = start.scope();
    let mut sends = Vec::new();
    let ran = Sandbox::boot(LIMITS).and_then(|sandbox| {
        sandbox.run(listen, &draft.code, &name, context, &mut |sub| {
            sends.push(planned(sub, &scope, &plan));
            Err(HttpError::Script(NOT_SENT.into()))
        })
    });
    report.sends = sends;
    match ran {
        Ok(ran) => {
            let mut after = start.clone();
            after.absorb(&ran.result);
            report.writes = draft_writes(&start, &after);
            (report.tests, report.console, report.error, report.skipped) = (ran.tests, ran.console, ran.error, ran.skip_request);
            if listen == "prerequest" {
                let changed = ran.result.get("request").map_or_else(|| saved.clone(), |r| request_after(&saved, r));
                match (shown(&mut collection, request_id, &saved, &start), shown(&mut collection, request_id, &changed, &after)) {
                    (Ok(before), Ok(after)) => report.request = request_changes(&before, &after),
                    (Err(e), _) | (_, Err(e)) => report.request_error = Some(e.to_string()),
                }
            }
        }
        // Past a limit: what it did up to there is not known.
        Err(e) => report.error = Some(e.to_string()),
    }

    let kept = ScriptDraft {
        collection: collection_id,
        request: request_id.into(),
        name: format!("{} / {name}", collection.name()),
        listen: listen.into(),
        chat: chat.into(),
        chat_title: chat_title.into(),
        environment: environment.name.clone(),
        code: draft.code.clone(),
        at: chrono::Utc::now().timestamp_millis(),
        report,
    };
    http_collections::save_draft(kept.clone())?;
    Ok(kept)
}

/// `request` as the page would send it with `vars`, as shown: a secret masked, and a
/// credential written into the collection by hand masked too, as the agent's mask does.
fn shown(collection: &mut Collection, id: &str, request: &Value, vars: &ScriptVars) -> Result<Outgoing, HttpError> {
    let item = collection.item_mut(id).ok_or_else(|| HttpError::NoRequest(id.into()))?;
    item.insert("request".into(), request.clone());
    let prepared = collection.prepare(id, &vars.scope())?;
    let literals = credential_literals(&prepared.sent);
    Ok(prepared.hidden(&vars.scope().keeping(literals.iter().map(String::as_str))).shown)
}

/// A `pm.sendRequest` as it would have gone: filled in where it can be, and whether a
/// hidden value is in it or its host is none of the collections'.
fn planned(sub: &Value, scope: &Scope, plan: &DraftPlan) -> DraftSend {
    let out = match one_request(sub, scope) {
        (_, Ok(prepared)) => prepared.shown,
        (url, Err(_)) => {
            let method = sub.get("method").and_then(Value::as_str).unwrap_or("GET").to_ascii_uppercase();
            Outgoing { method, url, headers: Vec::new(), body: None, digest: None }
        }
    };
    let masked = out.url.contains(SECRET_MASK) || out.headers.iter().any(|(_, v)| v.contains(SECRET_MASK)) || out.body.as_deref().is_some_and(|b| b.contains(SECRET_MASK));
    DraftSend { foreign: plan.foreign(&out.url), method: out.method, url: out.url, masked }
}
