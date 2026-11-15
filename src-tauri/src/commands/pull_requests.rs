//! The pull request strip in Changes and its servers (`docs/56-bitbucket-prs.md`),
//! through `services::pull_requests`. Everything here reaches a server, the
//! keychain or git, so every command but the watch's switches is `async`.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::commands::agent_state::AgentState;
use crate::domain::code_host::{CodeHostServer, PrStrip, PullRequest, PullRequestChanged, PullRequestEventSink};
use crate::infra::code_host_store;
use crate::services::pull_requests::PullRequests;

use super::blocking;

/// The strip of a folder changed — on the server, or by one of its buttons:
/// the window reads it again. Carries the folder it is about.
pub const PULL_REQUESTS_EVENT: &str = "pull-requests:changed";

#[derive(Clone, Serialize)]
struct Changed {
    root: String,
}

pub fn pull_request_event_sink<R: Runtime>(app: &AppHandle<R>) -> PullRequestEventSink {
    let app = app.clone();
    Arc::new(move |changed: PullRequestChanged| {
        let _ = app.emit(PULL_REQUESTS_EVENT, Changed { root: changed.root });
    })
}

type Prs<'a> = State<'a, Arc<PullRequests>>;
type Agent<'a> = State<'a, Arc<AgentState>>;

#[tauri::command]
pub async fn code_hosts() -> Result<Vec<CodeHostServer>, String> {
    blocking(code_host_store::list).await
}

/// Saves a server. `forFolder` is the connect form of the strip: the open
/// folder's repository is then proven to be on the server, and its host
/// learnt as an alias.
#[tauri::command]
pub async fn code_host_save(
    server: CodeHostServer,
    token: Option<String>,
    for_folder: bool,
    state: Agent<'_>,
    prs: Prs<'_>,
) -> Result<Vec<CodeHostServer>, String> {
    let root = if for_folder { Some(state.workspace()?) } else { None };
    let prs = prs.inner().clone();
    blocking(move || PullRequests::save(&prs, server, token, root.as_deref())).await
}

#[tauri::command]
pub async fn code_host_delete(id: String, prs: Prs<'_>) -> Result<Vec<CodeHostServer>, String> {
    let prs = prs.inner().clone();
    blocking(move || PullRequests::delete(&prs, &id)).await
}

/// What the address in the form answers as, without a token: "Bitbucket 8.19.26".
#[tauri::command]
pub async fn code_host_detect(server: CodeHostServer, prs: Prs<'_>) -> Result<String, String> {
    let prs = prs.inner().clone();
    blocking(move || PullRequests::identify(&prs, &server)).await
}

#[tauri::command]
pub async fn pull_request(state: Agent<'_>, prs: Prs<'_>) -> Result<PrStrip, String> {
    let root = state.workspace()?;
    let prs = prs.inner().clone();
    blocking(move || PullRequests::strip(&prs, &root)).await
}

#[tauri::command]
pub async fn pull_request_create(
    title: String,
    to: Option<String>,
    state: Agent<'_>,
    prs: Prs<'_>,
) -> Result<PullRequest, String> {
    let root = state.workspace()?;
    let prs = prs.inner().clone();
    blocking(move || PullRequests::create(&prs, &root, &title, to.as_deref())).await
}

/// `approved: false` takes the approval back.
#[tauri::command]
pub async fn pull_request_approve(approved: bool, state: Agent<'_>, prs: Prs<'_>) -> Result<(), String> {
    let root = state.workspace()?;
    let prs = prs.inner().clone();
    blocking(move || PullRequests::approve(&prs, &root, approved)).await
}

#[tauri::command]
pub async fn pull_request_merge(state: Agent<'_>, prs: Prs<'_>) -> Result<(), String> {
    let root = state.workspace()?;
    let prs = prs.inner().clone();
    blocking(move || PullRequests::merge(&prs, &root)).await
}

/// The strip is on screen: the open folder is watched until it says otherwise.
/// Starts a thread and asks nothing yet, so it may stay on the main thread.
#[tauri::command]
pub fn pull_request_watch(state: Agent<'_>, prs: Prs<'_>) -> Result<(), String> {
    prs.watch(state.workspace()?);
    Ok(())
}

/// `root` is the folder the strip watched, as the event names it — not the
/// open one: the strip of the folder just left unmounts after the next one
/// opened, and its unwatch must not stop the new watch.
#[tauri::command]
pub fn pull_request_unwatch(root: String, prs: Prs<'_>) {
    prs.unwatch(std::path::Path::new(&root));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(PULL_REQUESTS_EVENT, "pull-requests:changed");
    }

    /// The event names its folder, in the field the window's listener reads.
    #[test]
    fn the_event_carries_its_folder() {
        assert_eq!(serde_json::to_string(&Changed { root: "/a/b".to_string() }).unwrap(), r#"{"root":"/a/b"}"#);
    }
}
