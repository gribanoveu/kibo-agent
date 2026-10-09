//! Guards for the data policy (CA-12.6, `docs/08-data-policy.md`): the only
//! code that talks to the network is the model provider's.
//!
//! Tests over the source tree rather than a type-level fence, because what
//! has to be caught is the *next* change — a crate pulled in for one small
//! feature that happens to phone home, a `fetch` in a component. Each failure
//! names the file and says where the exception belongs if it is meant.

use std::fs;
use std::path::{Path, PathBuf};

/// The provider's client and the agent it is built on, the MCP servers the
/// user gave a URL (the SDK's client and the module that points it at that
/// URL), the cluster of a kubeconfig the user added, and the web search the
/// user turned on — Tavily, or DuckDuckGo and the pages it finds. Anything
/// else that opens a connection is a new place data can go, and the policy
/// document has to say so before this list does.
const NETWORK_ALLOWED: &[&str] = &[
    "src/infra/http_agent.rs",
    "src/infra/llm_providers/",
    "src/infra/mcp_http.rs",
    "src/infra/mcp_rmcp.rs",
    "src/infra/kube_client.rs",
    "src/infra/tavily.rs",
    "src/infra/open_web.rs",
];

/// What opening a connection looks like in Rust here.
const RUST_NETWORK: &[&str] =
    &["ureq::", "std::net::", "TcpStream", "UdpSocket", "reqwest::", "hyper::", "use kube::", " kube::", "(kube::", "k8s_openapi::", "use rmcp::", " rmcp::"];

/// And in the window. The CSP in `tauri.conf.json` refuses these at runtime
/// too; this catches them before anyone has to wonder why a request failed.
const WINDOW_NETWORK: &[&str] = &["fetch(", "XMLHttpRequest", "new WebSocket", "sendBeacon", "new EventSource"];

/// Crates and packages that exist to send data somewhere — HTTP clients
/// besides the one the provider uses, and telemetry. `reqwest` is in the
/// build all the same, under the MCP SDK (`rmcp`), which this list does not
/// see: it reads the manifest, not the graph. What holds there is the fence
/// above — `rmcp::` outside the two MCP modules fails.
const RUST_DENIED: &[&str] = &[
    "reqwest", "hyper", "isahc", "attohttpc", "surf", "curl", "tungstenite", "tokio-tungstenite", "sentry",
    "opentelemetry", "posthog", "tauri-plugin-http", "tauri-plugin-websocket", "tauri-plugin-upload",
];
const JS_DENIED: &[&str] = &[
    "axios", "@sentry/", "posthog", "mixpanel", "@segment/", "amplitude", "@datadog/", "logrocket",
    "@tauri-apps/plugin-http", "@tauri-apps/plugin-websocket", "@tauri-apps/plugin-upload",
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn files(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, extensions, out);
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| extensions.contains(&e)) {
            out.push(path);
        }
    }
}

/// Every line of `path` containing one of `needles` — comments too: a
/// mention is rare, and cheaper to reword than a real call is to miss.
fn hits(path: &Path, needles: &[&str]) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| needles.iter().any(|needle| line.contains(needle)))
        .map(|line| line.trim().to_string())
        .collect()
}

#[test]
fn only_the_provider_opens_connections() {
    let root = crate_dir();
    let mut sources = Vec::new();
    files(&root.join("src"), &["rs"], &mut sources);
    let this = root.join(file!().trim_start_matches("src-tauri/"));

    let offenders: Vec<String> = sources
        .iter()
        .filter(|path| **path != this)
        .filter(|path| {
            let relative = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            !NETWORK_ALLOWED.iter().any(|allowed| relative.starts_with(allowed))
        })
        .flat_map(|path| hits(path, RUST_NETWORK).into_iter().map(move |line| format!("{}: {line}", path.display())))
        .collect();
    assert!(
        offenders.is_empty(),
        "network code outside the provider — a new place data can go. Add it to docs/08-data-policy.md \
         and NETWORK_ALLOWED, or move it:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_window_sends_nothing_itself() {
    let mut sources = Vec::new();
    files(&crate_dir().join("../src"), &["ts", "tsx"], &mut sources);
    let offenders: Vec<String> = sources
        .iter()
        .filter(|path| !path.components().any(|c| c.as_os_str() == "__tests__"))
        .flat_map(|path| hits(path, WINDOW_NETWORK).into_iter().map(move |line| format!("{}: {line}", path.display())))
        .collect();
    assert!(offenders.is_empty(), "the window goes to the backend through invoke(), not the network:\n{}", offenders.join("\n"));
}

#[test]
fn no_dependency_exists_to_send_data_elsewhere() {
    let cargo = fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    let cargo = dependency_names(&cargo);
    let package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(crate_dir().join("../package.json")).unwrap()).unwrap();
    let js: Vec<&str> = ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|section| package[section].as_object())
        .flat_map(|deps| deps.keys().map(String::as_str))
        .collect();

    let denied: Vec<String> = cargo
        .iter()
        .filter(|name| RUST_DENIED.iter().any(|d| name == d || name.starts_with(&format!("{d}-"))))
        .cloned()
        .chain(js.iter().filter(|name| JS_DENIED.iter().any(|d| name.starts_with(d))).map(|n| n.to_string()))
        .collect();
    assert!(denied.is_empty(), "these exist to send data off the machine: {denied:?} — see docs/08-data-policy.md");
    assert!(!cargo.is_empty() && !js.is_empty(), "read no dependencies at all: the manifests moved?");
}

/// The window's own fence: whatever a page tries — a `fetch` added later, an
/// `<img>` in rendered model output pointing at someone's server — the
/// webview refuses any origin but the app's and its IPC.
///
/// Built app only. On desktop `tauri dev` loads the Vite server directly and
/// applies no CSP at all (Tauri proxies the dev server only on mobile), so
/// there is no `devCsp` to keep in step.
#[test]
fn the_window_may_reach_nothing_but_the_app() {
    let conf: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(crate_dir().join("tauri.conf.json")).unwrap()).unwrap();
    let csp = conf["app"]["security"]["csp"].as_object().expect("the CSP is not set");
    assert_eq!(csp["default-src"], "'self'");
    for (directive, sources) in csp {
        for source in sources.as_str().unwrap().split_whitespace() {
            // Exact sources, not a pattern: `https:` or `*` would pass any
            // rule that only looked for a host. `'wasm-unsafe-eval'` reaches
            // nothing: it lets the app's own WebAssembly compile — Graphviz,
            // which PlantUML diagrams are laid out with.
            let allowed = [
                "'self'", "'none'", "'unsafe-inline'", "'wasm-unsafe-eval'", "data:", "asset:", "ipc:",
                "http://ipc.localhost", "http://asset.localhost",
            ]
            .contains(&source);
            assert!(allowed, "csp {directive} lets the window reach {source}");
        }
    }
}

/// Dependency names from `Cargo.toml` without a TOML parser: a line
/// `name = …` under a `[…dependencies]` table.
fn dependency_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_deps = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_deps = line.trim_matches(['[', ']']).ends_with("dependencies");
        } else if let Some((name, _)) = line.split_once('=').filter(|_| in_deps && !line.starts_with('#')) {
            names.push(name.trim().trim_matches('"').to_string());
        }
    }
    names
}
