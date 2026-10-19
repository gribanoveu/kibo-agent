//! "Save as…" in the Preview panel (`docs/31-prototype-role.md`, decision 7):
//! a page the panel shows, as one HTML file any browser opens from disk.
//!
//! A browser has no `artifact://`, and loads no module from `file://`, so
//! every `/lib/…` the page names becomes a `data:` URL of that library. The
//! React modules are shims over one `react-all.js` they import relatively,
//! which a `data:` URL cannot do: they import it by a bare name instead, and
//! the page's import map gets that name — one copy, one React.

use std::fs;
use std::path::Path;

use base64::Engine as _;

use crate::domain::prototype::PrototypeError;
use crate::infra::prototype_protocol::{self, Roots};

/// The preview's rules, carried into the file: nothing the page loads, fetches
/// or posts leaves it — `form-action` and `base-uri` are two that `default-src`
/// does not cover. What no CSP can stop in a file of its own is the page
/// navigating itself away; in the panel the window's `frame-src` stops that.
const CSP: &str = "default-src 'unsafe-inline' data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'";

/// How a shim names `react-all.js`, as built by `scripts/build-prototype-libs.ts`.
const REACT_ALL: &str = "\"./react-all.js\"";
/// The bare name it is given in the file's import map.
const REACT_ALL_NAME: &str = "kibo-react-all";

/// Writes `page` — of the chat's working folder, or of version `version` —
/// to `dest`, the libraries it names inside it. `dest` comes from the
/// system's save dialog: it is the user's choice, not second-guessed.
pub fn export(roots: &Roots, chat_id: &str, version: Option<u32>, page: &str, dest: &Path) -> Result<(), PrototypeError> {
    let mut segments = vec![chat_id.to_string()];
    match version {
        Some(n) => segments.extend(["versions".to_string(), n.to_string()]),
        None => segments.push("files".to_string()),
    }
    segments.extend(page.split('/').map(str::to_string));
    let html = prototype_protocol::file(roots, &segments)
        .and_then(|file| fs::read(file).ok())
        .ok_or_else(|| PrototypeError::NoPage(page.to_string()))?;
    let lib = |name: &str| prototype_protocol::resolve(roots, &format!("/lib/{name}")).and_then(|file| fs::read(file).ok());
    let out = inline(&String::from_utf8_lossy(&html), lib);
    fs::write(dest, out).map_err(|source| PrototypeError::Io { path: dest.display().to_string(), source })
}

/// The page with every quoted `/lib/<name>` it names in place, and the CSP.
fn inline(page: &str, lib: impl Fn(&str) -> Option<Vec<u8>>) -> String {
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    let mut react_all = false;
    while let Some(at) = rest.find("/lib/") {
        let quote = at.checked_sub(1).map(|q| rest.as_bytes()[q]).filter(|q| matches!(q, b'"' | b'\''));
        let end = quote.and_then(|q| rest[at..].find(q as char).map(|end| at + end));
        match end.and_then(|end| lib(&rest[at + "/lib/".len()..end]).map(|bytes| (end, bytes))) {
            Some((end, bytes)) => {
                let (bytes, shim) = unshimmed(bytes);
                react_all |= shim;
                out.push_str(&rest[..at]);
                out.push_str(&data_url(&bytes));
                rest = &rest[end..];
            }
            None => {
                out.push_str(&rest[..at + "/lib/".len()]);
                rest = &rest[at + "/lib/".len()..];
            }
        }
    }
    out.push_str(rest);
    if react_all {
        if let Some(bytes) = lib("react-all.js") {
            out = with_import(&out, REACT_ALL_NAME, &data_url(&bytes));
        }
    }
    prototype_protocol::in_head(&out, &format!(r#"<meta http-equiv="Content-Security-Policy" content="{CSP}">"#))
}

/// A React shim importing `react-all.js` by its bare name; anything else as it is.
fn unshimmed(bytes: Vec<u8>) -> (Vec<u8>, bool) {
    match std::str::from_utf8(&bytes) {
        Ok(text) if text.contains(REACT_ALL) => (text.replace(REACT_ALL, &format!("\"{REACT_ALL_NAME}\"")).into_bytes(), true),
        _ => (bytes, false),
    }
}

fn data_url(script: &[u8]) -> String {
    format!("data:text/javascript;charset=utf-8;base64,{}", base64::engine::general_purpose::STANDARD.encode(script))
}

/// The page with `name` → `url` added to its import map; as it was when it
/// has none that reads as one.
fn with_import(page: &str, name: &str, url: &str) -> String {
    let lower = page.to_ascii_lowercase();
    let body = lower.find("importmap").and_then(|at| {
        let start = at + lower[at..].find('>')? + 1;
        Some((start, start + lower[start..].find("</script")?))
    });
    let Some((start, end)) = body else { return page.to_string() };
    let Ok(mut map) = serde_json::from_str::<serde_json::Value>(&page[start..end]) else { return page.to_string() };
    let Some(imports) = map.get_mut("imports").and_then(|imports| imports.as_object_mut()) else {
        return page.to_string();
    };
    imports.insert(name.to_string(), url.into());
    format!("{}{map}{}", &page[..start], &page[end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::path::PathBuf;

    const ID: &str = "0b9d6a0e-4f4e-4c8e-9a51-3c2f1d7e8a10";

    /// The React page the role's prompt starts from, as far as its libraries go.
    const PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8">
<script type="importmap">
{"imports": {
  "react": "/lib/react.js",
  "lucide-react": "/lib/lucide-react.js"
}}
</script>
<script src="/lib/tailwind.js"></script>
<script src='/lib/babel.min.js'></script>
</head><body><script type="text/babel" data-type="module">import { useState } from "react";</script></body></html>"#;

    fn roots(label: &str) -> (Roots, PathBuf) {
        let base = temp_dir(label);
        let chat = base.join("prototypes").join(ID);
        fs::create_dir_all(chat.join("files")).unwrap();
        fs::write(chat.join("files").join("index.html"), PAGE).unwrap();
        fs::create_dir_all(chat.join("versions").join("1")).unwrap();
        fs::write(chat.join("versions").join("1").join("index.html"), "<head><title>then</title></head>").unwrap();
        let libs = base.join("libs");
        fs::create_dir_all(&libs).unwrap();
        fs::write(libs.join("react.js"), "import { R } from \"./react-all.js\";\nexport default R;").unwrap();
        fs::write(libs.join("react-all.js"), "export const R = 'react';").unwrap();
        fs::write(libs.join("lucide-react.js"), "import \"react\";").unwrap();
        fs::write(libs.join("tailwind.js"), "/* tailwind */").unwrap();
        fs::write(libs.join("babel.min.js"), "/* babel */").unwrap();
        fs::write(base.join("secret.txt"), "key").unwrap();
        (Roots { app_dir: Some(base.clone()), libs: Some(libs) }, base)
    }

    fn decoded(url: &str) -> String {
        let data = url.strip_prefix("data:text/javascript;charset=utf-8;base64,").expect("a data URL");
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(data).unwrap()).unwrap()
    }

    fn exported(roots: &Roots, base: &Path, version: Option<u32>, page: &str) -> Result<String, PrototypeError> {
        let dest = base.join("out.html");
        export(roots, ID, version, page, &dest)?;
        Ok(fs::read_to_string(dest).unwrap())
    }

    fn import_map(page: &str) -> serde_json::Value {
        let start = page.find("importmap\">").unwrap() + "importmap\">".len();
        let end = start + page[start..].find("</script>").unwrap();
        serde_json::from_str(&page[start..end]).unwrap()
    }

    #[test]
    fn every_library_is_in_the_file_and_nothing_points_back_into_the_app() {
        let (roots, base) = roots("export-libs");
        let out = exported(&roots, &base, None, "index.html").unwrap();
        assert!(!out.contains("/lib/") && !out.contains("artifact:"), "{out}");
        let map = import_map(&out);
        let imports = &map["imports"];
        // The shim imports the one copy of React by the name the map gives it.
        assert_eq!(decoded(imports["react"].as_str().unwrap()), "import { R } from \"kibo-react-all\";\nexport default R;");
        assert_eq!(decoded(imports[REACT_ALL_NAME].as_str().unwrap()), "export const R = 'react';");
        assert_eq!(decoded(imports["lucide-react"].as_str().unwrap()), "import \"react\";");
        let script = |quote: char| {
            let at = out.find(&format!("src={quote}data:")).unwrap() + 5;
            decoded(&out[at..at + out[at..].find(quote).unwrap()])
        };
        assert_eq!((script('"'), script('\'')), ("/* tailwind */".to_string(), "/* babel */".to_string()));
        // The preview's rules, first in the head.
        assert!(out.contains(&format!("<head><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"><meta charset")));
        assert!(!CSP.contains("artifact") && CSP.contains("connect-src 'none'"));
        assert!(CSP.contains("form-action 'none'") && CSP.contains("base-uri 'none'"));
    }

    #[test]
    fn a_page_without_libraries_is_saved_as_it_is_but_for_the_csp() {
        let (roots, base) = roots("export-plain");
        let out = exported(&roots, &base, Some(1), "index.html").unwrap();
        assert_eq!(out, format!("<head><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"><title>then</title></head>"));
    }

    /// What is not a library the app bundles — a name out of `/lib/`, one it
    /// does not have, a path that only mentions it — is left as written.
    #[test]
    fn only_a_bundled_library_is_put_in() {
        let (roots, base) = roots("export-escape");
        // `secret.txt` is beside `libs/`: one `..` out.
        let page = r#"<script src="/lib/../secret.txt"></script><script src="/lib/nothing.js"></script><p>see /lib/react.js and</p>"#;
        fs::write(base.join("prototypes").join(ID).join("files").join("index.html"), page).unwrap();
        let out = exported(&roots, &base, None, "index.html").unwrap();
        assert_eq!(out, format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">{page}"));
    }

    #[test]
    fn a_page_out_of_the_folder_or_not_in_it_is_not_saved() {
        let (roots, base) = roots("export-no-page");
        for (version, page) in [(None, "missing.html"), (Some(2), "index.html"), (None, "../../../secret.txt"), (None, "../versions/1/index.html")] {
            assert!(matches!(exported(&roots, &base, version, page), Err(PrototypeError::NoPage(_))), "{version:?} {page}");
        }
        assert!(!base.join("out.html").exists());
    }

    #[test]
    fn a_destination_that_cannot_be_written_says_where() {
        let (roots, base) = roots("export-dest");
        let dest = base.join("no-such-dir").join("out.html");
        let err = export(&roots, ID, None, "index.html", &dest).unwrap_err();
        assert!(matches!(&err, PrototypeError::Io { path, .. } if path == &dest.display().to_string()), "{err}");
    }

    #[test]
    fn a_map_that_does_not_read_as_one_is_left_alone() {
        assert_eq!(with_import("<p>no map</p>", "a", "b"), "<p>no map</p>");
        let broken = r#"<script type="importmap">{"imports": </script>"#;
        assert_eq!(with_import(broken, "a", "b"), broken);
        let scopes = r#"<script type="importmap">{"scopes": {}}</script>"#;
        assert_eq!(with_import(scopes, "a", "b"), scopes);
        assert_eq!(
            with_import(r#"<script type="importmap">{"imports": {}}</script>"#, "a", "b"),
            r#"<script type="importmap">{"imports":{"a":"b"}}</script>"#
        );
    }
}
