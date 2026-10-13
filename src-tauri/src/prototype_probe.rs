//! P-0 of `docs/31-prototype-role.md`, kept to be run again: can a page in the
//! preview iframe reach the app? Debug builds only, and only with
//! `KIBO_PROTOTYPE_PROBE=<dir>` set. The probe's page goes into a chat folder
//! of its own and is served by the real `artifact://`; the main window gets a
//! sandboxed iframe from `<dir>/parent.js`; what the frame reports comes back
//! on a Tauri event and lands in `<dir>/report.jsonl`. How to run it is in
//! `scripts/prototype-probe/README.md`.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use tauri::{Listener, Manager};

use crate::infra::prototype_store;

const VAR: &str = "KIBO_PROTOTYPE_PROBE";
/// The chat id the probe's page is served under.
const CHAT: &str = "p0-probe";
const EVENT: &str = "prototype-probe";

pub fn start(app: &tauri::App) {
    let Some(dir) = std::env::var_os(VAR).map(PathBuf::from) else { return };
    let folder = match prototype_store::files_dir(CHAT) {
        Ok(folder) => folder,
        Err(e) => return eprintln!("[probe] no folder: {e}"),
    };
    for name in ["index.html", "checks.js"] {
        if let Err(e) = std::fs::copy(dir.join(name), folder.join(name)) {
            return eprintln!("[probe] {name} not copied: {e}");
        }
    }
    let Ok(script) = std::fs::read_to_string(dir.join("parent.js")) else {
        return eprintln!("[probe] no parent.js in {}", dir.display());
    };
    let report = dir.join("report.jsonl");
    app.listen_any(EVENT, move |event| {
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&report)
            .and_then(|mut f| writeln!(f, "{}", event.payload()));
        if let Err(e) = written {
            eprintln!("[probe] report not written: {e}");
        }
    });
    let app = app.handle().clone();
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
