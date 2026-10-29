//! The HTTP page's use cases over its two stores (`docs/45-http-client.md`):
//! a request sent and what came back kept, a Postman file imported into
//! whichever store it belongs to, and saved responses whose request is gone dropped.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde_json::Value;

use crate::domain::http_client::{HistoryEntry, HttpError, ResponseView, SavedResponse, Scope};
use crate::domain::postman::{read_import, Imported};
use crate::infra::{http_client, http_collections, http_env_store};

/// Sends a collection's request — `edited`, the page's unsaved `request`, in
/// place of the stored one — into `environment`. What came back is on disk, as
/// the request's last response and in history, before the window gets it.
/// A variable found nowhere fails here, before anything is sent.
pub fn send(collection_id: &str, request_id: &str, edited: Option<Value>, environment: Option<&str>) -> Result<ResponseView, HttpError> {
    let mut collection = http_collections::get(collection_id)?;
    if let Some(edited) = edited {
        let item = collection.item_mut(request_id).ok_or_else(|| HttpError::NoSuchItem(request_id.into()))?;
        item.insert("request".into(), edited);
    }
    let environment = environment.map(http_env_store::get).transpose()?;
    let globals = http_env_store::globals()?;
    let variables = collection.variables();
    let own = environment.as_ref().map(|e| e.variables.as_slice()).unwrap_or_default();
    let scope = Scope::new(&[&globals, &variables, own]);
    let prepared = collection.prepare(request_id, &scope)?;

    let key = format!("{collection_id}/{request_id}");
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    latest().insert(key.clone(), ticket);
    let (outcome, body) = http_client::send(&prepared.sent, environment.as_ref().and_then(|e| e.trusted_cert.as_deref()));
    let outcome = outcome.hidden(&scope);
    let at = chrono::Utc::now().timestamp_millis();
    let name = environment.map(|e| e.name);
    let written = collection.written_url(request_id).unwrap_or_default();
    // The request went out: a store that fails now must not hide what came back.
    if let Err(e) = http_collections::record(HistoryEntry::new(at, name.clone(), None, written, &prepared, outcome.clone(), &body)) {
        eprintln!("http: a send not written to history: {e}");
    }
    let saved = SavedResponse { at, environment: name, outcome, body };
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
        Imported::Collection(collection) => Ok(ImportedAs::Collection { id: http_collections::add(&collection)?, name: collection.name() }),
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

/// Run as the page opens: a collection imported again with other ids leaves
/// the old ids' responses behind.
pub fn prune_responses() -> Result<(), HttpError> {
    let known: HashMap<String, Option<HashSet<String>>> =
        http_collections::all()?.into_iter().map(|(id, collection)| (id, collection.ok().map(|c| c.request_ids().into_iter().collect()))).collect();
    http_collections::prune_responses(&known)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::{Environment, Outcome, Variable};
    use crate::domain::postman::Collection;
    use crate::infra::http_client::tests::{closed_port, reply, serve, Seen};
    use crate::testing::with_app_dir;
    use serde_json::json;
    use std::sync::mpsc::Receiver;

    const PROBE: &str = include_str!("../infra/testdata/postman/probe.postman_collection.json");

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
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new() };
            http_collections::save_response(&id, &request, &saved).unwrap();
            http_collections::save_response(&id, "removed-request", &saved).unwrap();
            http_collections::save_response("removed-collection", "r", &saved).unwrap();
            let ImportedAs::Collection { id: broken, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            http_collections::save_response(&broken, "r", &saved).unwrap();
            let file = http_collections::dir().unwrap().join("collections").join(format!("{broken}.postman_collection.json"));
            std::fs::write(file, "{ damaged").unwrap();
            prune_responses().unwrap();
            assert!(http_collections::last_response(&id, &request).unwrap().is_some());
            assert_eq!(http_collections::last_response(&id, "removed-request").unwrap(), None);
            assert_eq!(http_collections::last_response("removed-collection", "r").unwrap(), None);
            assert!(http_collections::last_response(&broken, "r").unwrap().is_some(), "a file that does not read keeps its responses");
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

    fn stage(base: String) -> String {
        let var = |key: &str, value: String, secret| Variable { key: key.into(), value, secret, enabled: true };
        let variables = vec![var("baseUrl", base, false), var("token", "s3cr3t-t0k".into(), true)];
        http_env_store::add(Environment { id: "stage".into(), name: "Stage".into(), variables, trusted_cert: None }).unwrap();
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
            http_env_store::add(Environment { id: "tls".into(), name: "TLS".into(), variables, trusted_cert: Some(CA.into()) }).unwrap();
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
}
