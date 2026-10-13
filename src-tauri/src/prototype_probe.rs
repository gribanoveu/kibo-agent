//! P-0 of `docs/31-prototype-role.md`: can a page in the preview iframe reach
//! the app? Debug builds only, and only with `KIBO_PROTOTYPE_PROBE=<dir>` set:
//! `<dir>` is served on `artifact://`, the main window gets two sandboxed
//! iframes from `<dir>/parent.js`, and what they report lands in
//! `<dir>/report.jsonl`. How to run it is in `scripts/prototype-probe/README.md`.
//!
//! Throwaway: the real protocol is P-2's, with its own path checks and tests.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::http::{Request, Response};
use tauri::{Builder, Manager, Runtime};

const VAR: &str = "KIBO_PROTOTYPE_PROBE";
const CSP: &str = "default-src artifact: 'unsafe-inline' 'unsafe-eval' data: blob:; connect-src 'none'";

fn dir() -> Option<PathBuf> {
    std::env::var_os(VAR).map(PathBuf::from).and_then(|d| d.canonicalize().ok())
}

pub fn install<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    let Some(dir) = dir() else { return builder };
    builder.register_uri_scheme_protocol("artifact", move |_ctx, request| serve(&dir, &request))
}

pub fn start(app: &tauri::AppHandle) {
    let Some(dir) = dir() else { return };
    let Ok(script) = std::fs::read_to_string(dir.join("parent.js")) else {
        eprintln!("[probe] no parent.js in {}", dir.display());
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        // ponytail: a fixed wait for the window's page to load; a probe, not a feature.
        std::thread::sleep(Duration::from_secs(6));
        match app.get_webview_window("main") {
            Some(window) => {
                if let Err(e) = window.eval(&script) {
                    eprintln!("[probe] eval failed: {e}");
                }
            }
            None => eprintln!("[probe] no main window"),
        }
    });
}

fn serve(dir: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path();
    let origin = request.headers().get("origin").and_then(|v| v.to_str().ok()).unwrap_or("-");
    eprintln!("[probe] {} {} origin={origin}", request.method(), path);
    if path == "/__probe" {
        let line = serde_json::json!({ "origin": origin, "body": String::from_utf8_lossy(request.body()) });
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("report.jsonl"))
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = written {
            eprintln!("[probe] report not written: {e}");
        }
        return respond(204, "text/plain", Vec::new(), false);
    }
    let file = dir.join(path.trim_start_matches('/'));
    match file.canonicalize() {
        Ok(file) if file.starts_with(dir) => match std::fs::read(&file) {
            Ok(bytes) => respond(200, mime(&file), bytes, !path.ends_with("nocsp.html")),
            Err(_) => respond(404, "text/plain", Vec::new(), false),
        },
        _ => respond(404, "text/plain", Vec::new(), false),
    }
}

fn mime(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn respond(status: u16, mime: &str, body: Vec<u8>, csp: bool) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", mime)
        // A sandboxed page's origin is opaque: its module imports come with `Origin: null`.
        .header("Access-Control-Allow-Origin", "*");
    if csp {
        response = response.header("Content-Security-Policy", CSP);
    }
    response.body(body).unwrap_or_default()
}
