//! `<app dir>/catalog/`: the last index read from each source, with its
//! `ETag` — so the page lists something without the network, and asking
//! again costs a `304` when nothing changed.
//!
//! One file per source, named by a hash of its address: the address is a
//! URL, and a URL is not a file name on every platform.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::infra::app_dir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedIndex {
    /// The address it was read from: a hash names the file, this says whose it is.
    pub source: String,
    pub etag: Option<String>,
    /// Unix seconds of the last answer, `200` or `304`.
    pub fetched_at: u64,
    /// The index as it came, parsed again on reading — the parser of the app
    /// reading it, not of the one that wrote it.
    pub text: String,
}

fn path(source: &str) -> Result<PathBuf, String> {
    let name = blake3::hash(source.as_bytes()).to_hex();
    Ok(app_dir::dir()?.join("catalog").join(format!("{}.json", &name[..16])))
}

/// `None` when nothing was kept, or what was kept no longer reads — either
/// way the source is asked as if for the first time.
pub fn load(source: &str) -> Option<CachedIndex> {
    let text = fs::read_to_string(path(source).ok()?).ok()?;
    serde_json::from_str::<CachedIndex>(&text).ok().filter(|cached| cached.source == source)
}

pub fn save(cached: &CachedIndex) -> Result<(), String> {
    let text = serde_json::to_string(cached).map_err(|e| e.to_string())?;
    app_dir::write_private(&path(&cached.source)?, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn cached(source: &str) -> CachedIndex {
        CachedIndex { source: source.into(), etag: Some("\"e\"".into()), fetched_at: 7, text: "{}".into() }
    }

    #[test]
    fn keeps_one_index_per_source() {
        with_app_dir("catalog-store", || {
            assert_eq!(load("https://a.example/"), None);
            save(&cached("https://a.example/")).unwrap();
            save(&cached("https://b.example/")).unwrap();
            assert_eq!(load("https://a.example/"), Some(cached("https://a.example/")));
            assert_eq!(load("https://b.example/").unwrap().source, "https://b.example/");
        });
    }

    #[test]
    fn a_file_that_no_longer_reads_is_nothing_kept() {
        with_app_dir("catalog-store-broken", || {
            let at = path("https://a.example/").unwrap();
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(&at, "{").unwrap();
            assert_eq!(load("https://a.example/"), None);
            // Another source's copy under this name — a collision, or a hand edit — is not this one's.
            fs::write(&at, serde_json::to_string(&cached("https://b.example/")).unwrap()).unwrap();
            assert_eq!(load("https://a.example/"), None);
        });
    }
}
