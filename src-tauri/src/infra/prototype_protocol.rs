//! `artifact://`: what the Preview panel shows (`docs/31-prototype-role.md`,
//! decision 4) — a Prototype chat's files and its presented versions, and the
//! libraries its pages import. Nothing else: a path resolves under one of
//! those roots or is a 404.
//!
//! ```text
//! /lib/<file>                          the bundled libraries
//! /<chat id>/files/<path>              the chat's working folder
//! /<chat id>/versions/<n>/<path>       a presented version
//! ```
//!
//! What a page served here may do is [`CSP`], sent with every response, and
//! the panel's `sandbox`. What keeps IPC from it is Tauri's invoke key alone:
//! the scheme is the app's own, so Tauri counts a page on it as local (P-0).

use std::fs;
use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use tauri::http::Response;

use crate::infra::prototype_store;

pub const SCHEME: &str = "artifact";

/// Its own scheme's files and inline code — Babel compiles the page's JSX into
/// an inline module, which needs no `eval` (checked by the probe) — and no
/// request out: no `fetch`, no image or font from another host.
/// `http://artifact.localhost` is the scheme's address on Windows.
pub const CSP: &str =
    "default-src artifact: http://artifact.localhost 'unsafe-inline' data: blob:; connect-src 'none'";

/// Where the protocol reads from; a root that is `None` serves nothing.
pub struct Roots {
    /// The app directory, holding `prototypes/<chat id>/`.
    pub app_dir: Option<PathBuf>,
    /// The bundled libraries (`scripts/build-prototype-libs.ts`).
    pub libs: Option<PathBuf>,
}

/// The bundled libraries, in the app's resources. A debug build that has not
/// copied them there, as `tauri dev` may not, reads them beside the sources,
/// where `scripts/build-prototype-libs.ts` writes them; a release build never
/// looks there — that path is the build machine's.
pub fn libs_dir(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let bundled = resource_dir.map(|dir| dir.join(LIBS));
    #[cfg(debug_assertions)]
    let bundled = bundled
        .filter(|dir| dir.join("react.js").is_file())
        .or_else(|| Some(Path::new(env!("CARGO_MANIFEST_DIR")).join(LIBS)));
    bundled
}

const LIBS: &str = "prototype-libs";

/// The response to a request for `path`, the URI's path as it came.
pub fn respond(roots: &Roots, path: &str) -> Response<Vec<u8>> {
    match resolve(roots, path).and_then(|file| fs::read(&file).ok().map(|bytes| (file, bytes))) {
        Some((file, bytes)) => {
            let mime = mime(&file);
            let bytes = if mime.starts_with("text/html") { with_reporter(bytes) } else { bytes };
            response(200, mime, bytes)
        }
        None => response(404, "text/plain; charset=utf-8", b"not found".to_vec()),
    }
}

/// Tells the Preview panel what went wrong on the page — a script that
/// threw, a promise nobody caught, JSX Babel could not read — so the user sees
/// why it is blank and can hand it to the model. Only the message goes, and
/// only to the window the frame is in. Served, not written: the page on disk,
/// and what "Save as" makes of it, are the model's alone.
const REPORTER: &str = r#"<script>(()=>{const say=(m)=>parent.postMessage({kibo:"page-error",message:String(m).slice(0,500)},"*");addEventListener("error",(e)=>say(e.error&&e.error.message||e.message));addEventListener("unhandledrejection",(e)=>say(e.reason&&e.reason.message||e.reason));})()</script>"#;

/// The page with [`REPORTER`] first in its head, before anything that could
/// fail: after `<head>`, or else after the doctype — never in front of it,
/// which would put the page in quirks mode.
fn with_reporter(page: Vec<u8>) -> Vec<u8> {
    let text = String::from_utf8_lossy(&page);
    let lower = text.to_ascii_lowercase();
    let tag_end = |tag: &str| lower.find(tag).and_then(|at| lower[at..].find('>').map(|end| at + end + 1));
    let at = tag_end("<head").or_else(|| tag_end("<!doctype")).unwrap_or(0);
    let mut served = String::with_capacity(text.len() + REPORTER.len());
    served.push_str(&text[..at]);
    served.push_str(REPORTER);
    served.push_str(&text[at..]);
    served.into_bytes()
}

/// The file `path` names, if it is one under its root.
fn resolve(roots: &Roots, path: &str) -> Option<PathBuf> {
    let segments = segments(path)?;
    let (first, rest) = segments.split_first()?;
    let (root, rest) = if first == "lib" {
        (roots.libs.clone()?, rest)
    } else {
        let chat = prototype_store::chat_dir(roots.app_dir.as_deref()?, first)?;
        match rest.split_first()? {
            (kind, rest) if kind == "files" => (chat.join("files"), rest),
            (kind, rest) if kind == "versions" => {
                let (n, rest) = rest.split_first()?;
                (chat.join("versions").join(n), rest)
            }
            _ => return None,
        }
    };
    // The one check that keeps a path in its root, canonical on both sides:
    // `..` in any spelling — `%2e%2e`, `..%2f`, a backslash on Windows — and a
    // symlink pointing out all resolve out of it, and are refused. What stays
    // inside is the chat's own; a directory or a name that is not there fails
    // to read.
    let root = root.canonicalize().ok()?;
    let file = rest.iter().fold(root.clone(), |path, segment| path.join(segment)).canonicalize().ok()?;
    file.starts_with(&root).then_some(file)
}

/// The path's segments, percent-decoded: a page names `my page.html` as
/// `my%20page.html`.
fn segments(path: &str) -> Option<Vec<String>> {
    path.trim_start_matches('/')
        .split('/')
        .map(|raw| percent_decode_str(raw).decode_utf8().ok().map(|segment| segment.into_owned()))
        .collect()
}

fn mime(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn response(status: u16, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Content-Security-Policy", CSP)
        // A sandboxed page's origin is opaque: its module imports come with
        // `Origin: null`. Nothing here is more than the page already shows.
        .header("Access-Control-Allow-Origin", "*")
        .header("X-Content-Type-Options", "nosniff")
        // The working folder changes under the same URL with every edit.
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    const ID: &str = "0b9d6a0e-4f4e-4c8e-9a51-3c2f1d7e8a10";

    /// An app dir with one chat — a page in its folder and in version 1 — and
    /// a library.
    fn roots(label: &str) -> (Roots, PathBuf) {
        let base = temp_dir(label);
        let chat = base.join("prototypes").join(ID);
        fs::create_dir_all(chat.join("files").join("assets")).unwrap();
        fs::write(chat.join("files").join("index.html"), "<h1>now</h1>").unwrap();
        fs::write(chat.join("files").join("assets").join("logo.svg"), "<svg/>").unwrap();
        fs::create_dir_all(chat.join("versions").join("1")).unwrap();
        fs::write(chat.join("versions").join("1").join("index.html"), "<h1>then</h1>").unwrap();
        let libs = base.join("libs");
        fs::create_dir_all(&libs).unwrap();
        fs::write(libs.join("react.js"), "export default 1;").unwrap();
        (Roots { app_dir: Some(base.clone()), libs: Some(libs) }, base)
    }

    fn body(response: &Response<Vec<u8>>) -> &str {
        std::str::from_utf8(response.body()).unwrap()
    }

    /// A page's body as written, the reporter in front of it taken off.
    fn served(response: &Response<Vec<u8>>) -> &str {
        body(response).strip_prefix(REPORTER).expect("a page starts with the reporter")
    }

    #[test]
    fn a_chats_page_its_version_and_a_library_are_served() {
        let (roots, _) = roots("protocol-served");
        let page = respond(&roots, &format!("/{ID}/files/index.html"));
        assert_eq!((page.status().as_u16(), served(&page)), (200, "<h1>now</h1>"));
        assert_eq!(page.headers()["Content-Type"], "text/html; charset=utf-8");
        let then = respond(&roots, &format!("/{ID}/versions/1/index.html"));
        assert_eq!(served(&then), "<h1>then</h1>");
        let svg = respond(&roots, &format!("/{ID}/files/assets/logo.svg"));
        assert_eq!(svg.headers()["Content-Type"], "image/svg+xml");
        let lib = respond(&roots, "/lib/react.js");
        assert_eq!((lib.status().as_u16(), lib.headers()["Content-Type"].to_str().unwrap()), (200, "text/javascript; charset=utf-8"));
    }

    /// A page is served with the reporter first in its head; a script or a
    /// stylesheet is served as it is.
    #[test]
    fn a_page_reports_its_errors_to_the_panel_and_nothing_else_is_touched() {
        let (roots, base) = roots("protocol-reporter");
        let page = body(&respond(&roots, &format!("/{ID}/files/index.html"))).to_string();
        assert_eq!(page, format!("{REPORTER}<h1>now</h1>"));
        fs::write(base.join("libs").join("react.js"), "<head>").unwrap();
        assert_eq!(body(&respond(&roots, "/lib/react.js")), "<head>");
    }

    #[test]
    fn the_reporter_goes_after_the_head_tag_or_the_doctype_never_before_it() {
        let put = |page: &str| String::from_utf8(with_reporter(page.as_bytes().to_vec())).unwrap();
        assert_eq!(
            put("<!doctype html><html><HEAD lang=\"en\"><title>t</title></head></html>"),
            format!("<!doctype html><html><HEAD lang=\"en\">{REPORTER}<title>t</title></head></html>")
        );
        assert_eq!(put("<!DOCTYPE html>\n<p>no head</p>"), format!("<!DOCTYPE html>{REPORTER}\n<p>no head</p>"));
        assert_eq!(put("<p>bare</p>"), format!("{REPORTER}<p>bare</p>"));
        // Only to the frame's own window, and only the message.
        assert!(REPORTER.contains("parent.postMessage") && !REPORTER.contains("top."));
    }

    /// Every response carries the preview's rules — a 404 too, so nothing the
    /// frame loads is without them.
    /// A name is decoded before it is looked for.
    #[test]
    fn an_encoded_name_is_found() {
        let (roots, base) = roots("protocol-encoded");
        fs::write(base.join("prototypes").join(ID).join("files").join("my page.html"), "spaced").unwrap();
        let page = respond(&roots, &format!("/{ID}/files/my%20page.html"));
        assert_eq!((page.status().as_u16(), served(&page)), (200, "spaced"));
    }

    #[test]
    fn every_response_carries_the_csp_and_lets_an_opaque_origin_import() {
        let (roots, _) = roots("protocol-headers");
        for path in [format!("/{ID}/files/index.html"), "/nothing".to_string()] {
            let response = respond(&roots, &path);
            assert_eq!(response.headers()["Content-Security-Policy"], CSP, "{path}");
            assert_eq!(response.headers()["Access-Control-Allow-Origin"], "*", "{path}");
            assert_eq!(response.headers()["Cache-Control"], "no-store", "{path}");
        }
        assert!(CSP.contains("connect-src 'none'"));
        // Babel needs none, and a page that may `eval` may build code from anything it reads.
        assert!(!CSP.contains("'unsafe-eval'"));
    }

    /// The ways out of a root, each a 404 — and so is a root that is fine but
    /// a name it does not have.
    #[test]
    fn a_path_out_of_its_root_or_to_nothing_is_not_found() {
        let (roots, base) = roots("protocol-escape");
        fs::write(base.join("secret.txt"), "key").unwrap();
        fs::write(base.join("prototypes").join(ID).join("outside.txt"), "x").unwrap();
        for path in [
            format!("/{ID}/files/../../../secret.txt"),
            format!("/{ID}/files/%2e%2e/outside.txt"),
            format!("/{ID}/files/%2E%2E/%2E%2E/%2E%2E/secret.txt"),
            format!("/{ID}/files/..%2f..%2foutside.txt"),
            format!("/{ID}/files/assets%5c..%5c..%5coutside.txt"),
            format!("/{ID}/outside.txt"),
            // The chat's folder holds `files/` and `versions/`; nothing else in it is served.
            format!("/{ID}/meta/../outside.txt"),
            format!("/{ID}/x/outside.txt"),
            format!("/{ID}/versions/x/index.html"),
            format!("/{ID}/versions/1"),
            format!("/{ID}/files/"),
            format!("/{ID}/files"),
            "/../secret.txt".to_string(),
            "/lib/../secret.txt".to_string(),
            "/not-a-chat/files/index.html".to_string(),
            "/a%2fb/files/index.html".to_string(),
            "/".to_string(),
            "".to_string(),
        ] {
            assert_eq!(respond(&roots, &path).status().as_u16(), 404, "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_folder_is_not_followed() {
        let (roots, base) = roots("protocol-symlink");
        fs::write(base.join("secret.txt"), "key").unwrap();
        let files = base.join("prototypes").join(ID).join("files");
        std::os::unix::fs::symlink(base.join("secret.txt"), files.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&base, files.join("up")).unwrap();
        assert_eq!(respond(&roots, &format!("/{ID}/files/link.txt")).status().as_u16(), 404);
        assert_eq!(respond(&roots, &format!("/{ID}/files/up/secret.txt")).status().as_u16(), 404);
    }

    /// The bundled copy when there is one; a debug build falls back to the
    /// sources' copy.
    #[test]
    fn the_libraries_are_the_bundled_ones_and_a_debug_build_falls_back_to_the_sources() {
        let resources = temp_dir("protocol-libs");
        fs::create_dir_all(resources.join(LIBS)).unwrap();
        fs::write(resources.join(LIBS).join("react.js"), "").unwrap();
        assert_eq!(libs_dir(Some(&resources)), Some(resources.join(LIBS)));
        let sources = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join(LIBS));
        assert_eq!(libs_dir(Some(&temp_dir("protocol-no-libs"))), sources);
        assert_eq!(libs_dir(None), sources);
    }

    #[test]
    fn a_missing_root_serves_nothing() {
        let roots = Roots { app_dir: None, libs: None };
        assert_eq!(respond(&roots, &format!("/{ID}/files/index.html")).status().as_u16(), 404);
        assert_eq!(respond(&roots, "/lib/react.js").status().as_u16(), 404);
    }

    /// P-0 found that only the invoke key keeps a preview page from IPC, and
    /// that holds for the Tauri and wry it ran against. A new one is
    /// unchecked until the probe runs again.
    #[test]
    fn tauri_and_wry_are_the_versions_the_preview_probe_checked() {
        const PROBED: &[(&str, &str)] = &[("tauri", "2.11.5"), ("wry", "0.55.1")];
        let lock = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).unwrap();
        for (name, probed) in PROBED {
            let version = lock
                .split("[[package]]")
                .find(|package| package.contains(&format!("\nname = \"{name}\"\n")))
                .and_then(|package| package.lines().find_map(|line| line.strip_prefix("version = \"")))
                .map(|rest| rest.trim_end_matches('"'))
                .unwrap_or_else(|| panic!("{name} is not in Cargo.lock"));
            assert_eq!(
                version, *probed,
                "{name} is now {version}: run the preview probe again (scripts/prototype-probe/README.md), \
                 and once its report holds, set PROBED here and the versions in docs/31-prototype-role.md"
            );
        }
    }
}
