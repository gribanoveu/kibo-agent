//! The catalog page's list (`docs/32-catalog.md`): every source asked for its
//! index, with what was kept from last time standing in when the source
//! cannot answer — or answers with something this app cannot read.

use serde::Serialize;
use url::Url;

use crate::domain::catalog::{self, CatalogError, CatalogItem};
use crate::infra::catalog_http::{self, Fetched};
use crate::infra::catalog_store::{self, CachedIndex};
use crate::infra::settings_store;

/// One source as the page shows it. `error` beside `items` means the items
/// are the copy from `fetched_at`, and why nothing newer is shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub url: String,
    /// The app's own catalog, [`catalog::DEFAULT_SOURCE`]: always there.
    pub built_in: bool,
    pub items: Vec<CatalogItem>,
    /// Entries of the index left out, each with why.
    pub problems: Vec<String>,
    pub error: Option<String>,
    /// Unix seconds of the copy shown; `None` while there is none.
    pub fetched_at: Option<u64>,
}

type Fetch<'a> = &'a dyn Fn(&Url, Option<&str>) -> Result<Fetched, CatalogError>;

/// Every source, each asked once — a conditional GET when a copy is kept.
pub fn list() -> Vec<SourceView> {
    let added = settings_store::load().map(|settings| settings.catalog_sources).unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    sources(&added).iter().map(|source| read(source, &catalog_http::fetch_index, now)).collect()
}

/// The app's own first, then the user's in their order, each once: two
/// spellings of one address (`…/main` and `…/main/`) are one source.
fn sources(added: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut listed = Vec::new();
    for source in std::iter::once(catalog::DEFAULT_SOURCE).chain(added.iter().map(String::as_str)) {
        let key = catalog::source_base(source).map_or_else(|_| source.trim().to_string(), String::from);
        if !seen.contains(&key) {
            seen.push(key);
            listed.push(source.trim().to_string());
        }
    }
    listed
}

fn read(source: &str, fetch: Fetch, now: u64) -> SourceView {
    let mut view = SourceView { url: source.to_string(), built_in: source == catalog::DEFAULT_SOURCE, ..SourceView::default() };
    let base = match catalog::source_base(source) {
        Ok(base) => base,
        Err(e) => {
            view.error = Some(e.to_string());
            return view;
        }
    };
    let key = base.as_str();
    let kept = catalog_store::load(key);
    let shown = match fetch(&base, kept.as_ref().and_then(|k| k.etag.as_deref())) {
        Ok(Fetched::Fresh { text, etag }) => match catalog::parse_index(&text) {
            Ok(_) => Some(keep(CachedIndex { source: key.to_string(), etag, fetched_at: now, text })),
            Err(e) => {
                view.error = Some(e.to_string());
                kept
            }
        },
        // Only ever an answer to an ETag sent, so there is a copy to show.
        Ok(Fetched::NotModified) => kept.map(|k| keep(CachedIndex { fetched_at: now, ..k })),
        Err(e) => {
            view.error = Some(e.to_string());
            kept
        }
    };
    let Some(shown) = shown else { return view };
    view.fetched_at = Some(shown.fetched_at);
    match catalog::parse_index(&shown.text) {
        Ok(index) => {
            view.items = index.items;
            view.problems = index.problems;
        }
        // A copy kept by an older app, in a format this one no longer reads.
        Err(e) => view.error = Some(e.to_string()),
    }
    view
}

/// Written for next time; a copy that fails to write costs only that, so the
/// list shown now is not refused for it.
fn keep(cached: CachedIndex) -> CachedIndex {
    let _ = catalog_store::save(&cached);
    cached
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use std::cell::RefCell;

    const BASE: &str = "https://cat.example/main/";
    const ONE: &str = r#"{"version": 1, "items": [
        {"id": "changelog", "kind": "command", "name": "/changelog", "rev": "1", "path": "commands/changelog.md"},
        {"id": "x", "kind": "agent", "name": "X", "rev": "1", "path": "x.md"}]}"#;

    fn fresh(text: &str, etag: &str) -> Result<Fetched, CatalogError> {
        Ok(Fetched::Fresh { text: text.into(), etag: Some(etag.into()) })
    }

    const DEFAULT_TRIMMED: &str = "https://raw.githubusercontent.com/gribanoveu/kibo-agent-catalog/main";

    #[test]
    fn the_apps_own_comes_first_and_each_source_once() {
        let added = [" https://cat.example/main ".to_string(), "https://cat.example/main/".into(), DEFAULT_TRIMMED.into(), "nonsense".into()];
        assert_eq!(sources(&added), [catalog::DEFAULT_SOURCE, "https://cat.example/main", "nonsense"]);
    }

    #[test]
    fn a_fresh_index_is_listed_kept_and_asked_for_by_its_etag_next() {
        with_app_dir("catalog-fresh", || {
            let view = read(BASE, &|_, _| fresh(ONE, "\"v1\""), 100);
            assert_eq!(view.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["changelog"]);
            assert_eq!(view.problems.len(), 1);
            assert_eq!((view.error, view.fetched_at, view.built_in), (None, Some(100), false));

            let asked = RefCell::new(None);
            let again = read(
                BASE,
                &|url, etag| {
                    *asked.borrow_mut() = Some((url.to_string(), etag.map(str::to_string)));
                    Ok(Fetched::NotModified)
                },
                200,
            );
            assert_eq!(asked.into_inner(), Some((BASE.to_string(), Some("\"v1\"".to_string()))));
            assert_eq!((again.items.len(), again.fetched_at, again.error), (1, Some(200), None));
            // The time of the 304 is kept too.
            assert_eq!(catalog_store::load(BASE).unwrap().fetched_at, 200);
        });
    }

    #[test]
    fn without_the_network_the_kept_copy_is_shown_with_why() {
        with_app_dir("catalog-offline", || {
            let first = read(BASE, &|_, _| Err(CatalogError::Unavailable("no route".into())), 100);
            assert_eq!((first.items.len(), first.fetched_at), (0, None));
            assert!(first.error.unwrap().contains("no route"));

            read(BASE, &|_, _| fresh(ONE, "\"v1\""), 100);
            let offline = read(BASE, &|_, _| Err(CatalogError::Status(503)), 300);
            assert_eq!((offline.items.len(), offline.fetched_at), (1, Some(100)));
            assert_eq!(offline.error.as_deref(), Some("the catalog answered 503"));
        });
    }

    #[test]
    fn an_index_that_does_not_read_keeps_the_last_good_one() {
        with_app_dir("catalog-broken", || {
            read(BASE, &|_, _| fresh(ONE, "\"v1\""), 100);
            let broken = read(BASE, &|_, _| fresh("<html>rate limited</html>", "\"v2\""), 200);
            assert_eq!((broken.items.len(), broken.fetched_at), (1, Some(100)));
            assert!(broken.error.unwrap().contains("not JSON"));
            // Not written over: the next ask sends the good copy's ETag.
            assert_eq!(catalog_store::load(BASE).unwrap().etag.as_deref(), Some("\"v1\""));

            let newer = read(BASE, &|_, _| fresh(r#"{"version": 2}"#, "\"v3\""), 300);
            assert!(newer.error.unwrap().contains("newer app"));
            assert_eq!(newer.items.len(), 1);
        });
    }

    #[test]
    fn a_source_that_is_not_an_address_is_never_asked() {
        with_app_dir("catalog-bad-source", || {
            let view = read("http://cat.example/", &|_, _| panic!("asked"), 100);
            assert!(view.error.unwrap().contains("only for this machine"));
            assert!(view.items.is_empty());
        });
    }

    #[test]
    fn the_apps_own_is_marked() {
        with_app_dir("catalog-own", || {
            assert!(read(catalog::DEFAULT_SOURCE, &|_, _| Err(CatalogError::Status(500)), 1).built_in);
        });
    }
}
