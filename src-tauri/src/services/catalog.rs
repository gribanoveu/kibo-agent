//! The catalog page (`docs/32-catalog.md`): every source asked for its index,
//! with what was kept from last time standing in when the source cannot
//! answer — or answers with something this app cannot read; and installing
//! and removing an entry.

use std::path::PathBuf;

use serde::Serialize;
use url::Url;

use crate::domain::catalog::{self, CatalogError, CatalogItem, Installed, ItemKind, ItemState};
use crate::domain::runbooks;
use crate::infra::catalog_http::{self, Fetched};
use crate::infra::catalog_store::{self, CachedIndex};
use crate::infra::{app_dir, settings_store};

/// An entry as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    #[serde(flatten)]
    pub item: CatalogItem,
    pub state: ItemState,
    /// Where it goes, as a path under the home folder reads
    /// (`~/.kibo/skills/review`); `None` for a kind not installed as files yet.
    pub target: Option<String>,
    /// A runbook named as one the app ships: installed, it is read instead.
    pub replaces_built_in: bool,
}

/// One source as the page shows it. `error` beside `items` means the items
/// are the copy from `fetched_at`, and why nothing newer is shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub url: String,
    /// The app's own catalog, [`catalog::DEFAULT_SOURCE`]: always there.
    pub built_in: bool,
    pub items: Vec<ItemView>,
    /// Entries of the index left out, each with why.
    pub problems: Vec<String>,
    pub error: Option<String>,
    /// Unix seconds of the copy shown; `None` while there is none.
    pub fetched_at: Option<u64>,
}

type Fetch<'a> = &'a dyn Fn(&Url, Option<&str>) -> Result<Fetched, CatalogError>;
type FetchFile<'a> = &'a dyn Fn(&Url, &str) -> Result<Vec<u8>, CatalogError>;

/// Every source, each asked once — a conditional GET when a copy is kept.
pub fn list() -> Vec<SourceView> {
    let added = settings_store::load().map(|settings| settings.catalog_sources).unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // Unreadable, every entry in a place reads as taken: nothing offers to
    // install over, or remove, what the record would have said.
    let installed = catalog_store::load_installed().unwrap_or_default();
    sources(&added).iter().map(|source| read(source, &catalog_http::fetch_index, now, &installed)).collect()
}

/// The record of `kind`/`id`, whichever source it came from: the place is one per kind and id.
fn record<'a>(installed: &'a [Installed], kind: ItemKind, id: &str) -> Option<&'a Installed> {
    installed.iter().find(|record| record.kind == kind && record.id == id)
}

/// Where `relative` (from [`catalog::target`]) is on this machine.
fn place_of(relative: &std::path::Path) -> Result<PathBuf, CatalogError> {
    app_dir::dir().map(|dir| dir.join(relative)).map_err(CatalogError::Disk)
}

fn item_view(item: CatalogItem, source: &str, installed: &[Installed]) -> ItemView {
    let relative = catalog::target(item.kind, &item.id, item.role.as_deref());
    let record = record(installed, item.kind, &item.id);
    let state = match relative.as_deref().map(place_of) {
        Some(Ok(target)) => {
            let files = record.map_or(&item.files, |record| &record.files);
            catalog::state(&item, source, record, catalog_store::hash_at(&target, item.kind == ItemKind::Skill, files).as_deref())
        }
        // MCP and hooks: installed into a config file, from CT-3 on.
        _ => ItemState::Available,
    };
    ItemView {
        target: relative.map(|path| format!("~/.kibo/{}", path.to_string_lossy().replace('\\', "/"))),
        replaces_built_in: item.kind == ItemKind::Runbook && runbooks::is_built_in(&item.id),
        state,
        item,
    }
}

/// The entry as the source's kept index lists it — the copy the page shows,
/// so what is installed is what was looked at.
fn listed(source: &str, kind: ItemKind, id: &str) -> Result<(Url, CatalogItem), CatalogError> {
    let base = catalog::source_base(source)?;
    let kept = catalog_store::load(base.as_str()).ok_or_else(|| CatalogError::NotListed(id.to_string()))?;
    let item = catalog::parse_index(&kept.text)?
        .items
        .into_iter()
        .find(|item| item.kind == kind && item.id == id)
        .ok_or_else(|| CatalogError::NotListed(id.to_string()))?;
    Ok((base, item))
}

/// Installs a skill, command or runbook: every file fetched first, then
/// written into its place in one go, then recorded.
pub fn install(source: &str, kind: ItemKind, id: &str) -> Result<(), CatalogError> {
    install_with(source, kind, id, &catalog_http::fetch_file)
}

fn install_with(source: &str, kind: ItemKind, id: &str, fetch: FetchFile) -> Result<(), CatalogError> {
    let (base, item) = listed(source, kind, id)?;
    let relative = catalog::target(kind, id, item.role.as_deref()).ok_or_else(|| CatalogError::NotYet(item.name.clone()))?;
    let target = place_of(&relative)?;
    let mut installed = catalog_store::load_installed().map_err(CatalogError::Record)?;
    let folder = kind == ItemKind::Skill;
    let at = catalog_store::hash_at(&target, folder, &item.files);
    if catalog::state(&item, base.as_str(), record(&installed, kind, id), at.as_deref()) != ItemState::Available {
        return Err(CatalogError::Present(item.name));
    }
    let files: Vec<(String, Vec<u8>)> = if folder {
        item.files.iter().map(|file| Ok((file.clone(), fetch(&base, &format!("{}/{file}", item.path))?))).collect::<Result<_, CatalogError>>()?
    } else {
        vec![(String::new(), fetch(&base, &item.path)?)]
    };
    catalog_store::place(&target, folder, &files).map_err(CatalogError::Disk)?;
    let hash = catalog::content_hash(&files.iter().map(|(name, bytes)| (name.as_str(), bytes.as_slice())).collect::<Vec<_>>());
    // A record left by an install deleted by hand since is replaced.
    installed.retain(|record| !(record.kind == kind && record.id == id));
    installed.push(Installed {
        source: base.to_string(),
        kind,
        id: id.to_string(),
        role: item.role,
        rev: item.rev,
        files: if folder { item.files } else { Vec::new() },
        hash,
    });
    catalog_store::save_installed(&installed).map_err(|e| {
        // Unrecorded, it would read as someone else's and never come off from here.
        let _ = catalog_store::remove(&target, folder);
        CatalogError::Record(e)
    })
}

/// Removes what this source installed as `kind`/`id`, from where the record
/// says — the catalog may no longer list it.
pub fn remove(source: &str, kind: ItemKind, id: &str) -> Result<(), CatalogError> {
    let base = catalog::source_base(source)?;
    let mut installed = catalog_store::load_installed().map_err(CatalogError::Record)?;
    let record = record(&installed, kind, id)
        .filter(|record| record.source == base.as_str())
        .ok_or_else(|| CatalogError::NotInstalled(id.to_string()))?;
    let relative = catalog::target(kind, id, record.role.as_deref()).ok_or_else(|| CatalogError::NotInstalled(id.to_string()))?;
    catalog_store::remove(&place_of(&relative)?, kind == ItemKind::Skill).map_err(CatalogError::Disk)?;
    installed.retain(|record| !(record.kind == kind && record.id == id));
    catalog_store::save_installed(&installed).map_err(CatalogError::Record)
}

/// The text the details panel shows: a skill's `SKILL.md`, every other
/// kind's one file — read from the source, as an install would.
pub fn preview(source: &str, kind: ItemKind, id: &str) -> Result<String, CatalogError> {
    preview_with(source, kind, id, &catalog_http::fetch_file)
}

fn preview_with(source: &str, kind: ItemKind, id: &str, fetch: FetchFile) -> Result<String, CatalogError> {
    let (base, item) = listed(source, kind, id)?;
    let path = if kind == ItemKind::Skill { format!("{}/{}", item.path, catalog::SKILL_MD) } else { item.path };
    Ok(String::from_utf8_lossy(&fetch(&base, &path)?).into_owned())
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

fn read(source: &str, fetch: Fetch, now: u64, installed: &[Installed]) -> SourceView {
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
            view.items = index.items.into_iter().map(|item| item_view(item, key, installed)).collect();
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
            let view = read(BASE, &|_, _| fresh(ONE, "\"v1\""), 100, &[]);
            assert_eq!(view.items.iter().map(|i| i.item.id.as_str()).collect::<Vec<_>>(), ["changelog"]);
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
                &[],
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
            let first = read(BASE, &|_, _| Err(CatalogError::Unavailable("no route".into())), 100, &[]);
            assert_eq!((first.items.len(), first.fetched_at), (0, None));
            assert!(first.error.unwrap().contains("no route"));

            read(BASE, &|_, _| fresh(ONE, "\"v1\""), 100, &[]);
            let offline = read(BASE, &|_, _| Err(CatalogError::Status(503)), 300, &[]);
            assert_eq!((offline.items.len(), offline.fetched_at), (1, Some(100)));
            assert_eq!(offline.error.as_deref(), Some("the catalog answered 503"));
        });
    }

    #[test]
    fn an_index_that_does_not_read_keeps_the_last_good_one() {
        with_app_dir("catalog-broken", || {
            read(BASE, &|_, _| fresh(ONE, "\"v1\""), 100, &[]);
            let broken = read(BASE, &|_, _| fresh("<html>rate limited</html>", "\"v2\""), 200, &[]);
            assert_eq!((broken.items.len(), broken.fetched_at), (1, Some(100)));
            assert!(broken.error.unwrap().contains("not JSON"));
            // Not written over: the next ask sends the good copy's ETag.
            assert_eq!(catalog_store::load(BASE).unwrap().etag.as_deref(), Some("\"v1\""));

            let newer = read(BASE, &|_, _| fresh(r#"{"version": 2}"#, "\"v3\""), 300, &[]);
            assert!(newer.error.unwrap().contains("newer app"));
            assert_eq!(newer.items.len(), 1);
        });
    }

    #[test]
    fn a_source_that_is_not_an_address_is_never_asked() {
        with_app_dir("catalog-bad-source", || {
            let view = read("http://cat.example/", &|_, _| panic!("asked"), 100, &[]);
            assert!(view.error.unwrap().contains("only for this machine"));
            assert!(view.items.is_empty());
        });
    }

    #[test]
    fn the_apps_own_is_marked() {
        with_app_dir("catalog-own", || {
            assert!(read(catalog::DEFAULT_SOURCE, &|_, _| Err(CatalogError::Status(500)), 1, &[]).built_in);
        });
    }

    const CAT: &str = r#"{"version": 1, "items": [
        {"id": "review", "kind": "skill", "name": "Code review", "rev": "1", "path": "skills/review", "files": ["SKILL.md", "refs/list.md"]},
        {"id": "changelog", "kind": "command", "name": "/changelog", "rev": "1", "path": "commands/changelog.md"},
        {"id": "crashloop", "kind": "runbook", "role": "kubernetes", "name": "Crash loop", "rev": "1", "path": "runbooks/kubernetes/crashloop.md"},
        {"id": "github", "kind": "mcp", "name": "GitHub", "rev": "1", "path": "mcp/github.json"},
        {"id": "tls", "kind": "runbook", "role": "kubernetes", "name": "TLS", "rev": "1", "path": "runbooks/kubernetes/tls.md"},
        {"id": "probes", "kind": "command", "name": "/probes", "rev": "1", "path": "commands/probes.md"}]}"#;

    /// The source's files, by path under it.
    fn files(url: &Url, path: &str) -> Result<Vec<u8>, CatalogError> {
        assert!(url.as_str() == BASE, "{url}");
        match path {
            "skills/review/SKILL.md" => Ok(b"# Review".to_vec()),
            "skills/review/refs/list.md" => Ok(b"- bugs".to_vec()),
            "commands/changelog.md" => Ok(b"Add a line".to_vec()),
            "runbooks/kubernetes/crashloop.md" => Ok(b"Sign: mine".to_vec()),
            _ => Err(CatalogError::Status(404)),
        }
    }

    /// The catalog listed once, as the page would have: what install reads from.
    fn listed_once(text: &str) -> Vec<ItemView> {
        read(BASE, &|_, _| fresh(text, "\"v1\""), 100, &[]).items
    }

    fn states(installed: &[Installed]) -> Vec<(String, ItemState)> {
        read(BASE, &|_, _| Ok(Fetched::NotModified), 200, installed).items.into_iter().map(|v| (v.item.id, v.state)).collect()
    }

    fn here(relative: &str) -> PathBuf {
        app_dir::dir().unwrap().join(relative)
    }

    #[test]
    fn rows_say_where_each_kind_goes_and_what_a_runbook_replaces() {
        with_app_dir("catalog-views", || {
            let items = listed_once(CAT);
            let at = |id: &str| items.iter().find(|v| v.item.id == id).unwrap();
            assert_eq!(at("review").target.as_deref(), Some("~/.kibo/skills/review"));
            assert_eq!(at("crashloop").target.as_deref(), Some("~/.kibo/runbooks/kubernetes/crashloop.md"));
            assert_eq!(at("github").target, None);
            assert!(at("crashloop").replaces_built_in);
            assert!(!at("tls").replaces_built_in);
            // A built-in runbook's name on another kind replaces nothing.
            assert!(!at("probes").replaces_built_in);
            assert!(items.iter().all(|v| v.state == ItemState::Available));
        });
    }

    #[test]
    fn install_writes_each_kind_into_its_place_and_remove_takes_it_back() {
        with_app_dir("catalog-install", || {
            listed_once(CAT);
            for (kind, id) in [(ItemKind::Skill, "review"), (ItemKind::Command, "changelog"), (ItemKind::Runbook, "crashloop")] {
                install_with(BASE, kind, id, &files).unwrap();
            }
            assert_eq!(std::fs::read_to_string(here("skills/review/refs/list.md")).unwrap(), "- bugs");
            assert_eq!(std::fs::read_to_string(here("commands/changelog.md")).unwrap(), "Add a line");
            assert_eq!(std::fs::read_to_string(here("runbooks/kubernetes/crashloop.md")).unwrap(), "Sign: mine");
            let installed = catalog_store::load_installed().unwrap();
            assert_eq!(installed.len(), 3);
            assert_eq!(installed[0].files, ["SKILL.md", "refs/list.md"]);
            assert_eq!(installed[2].role.as_deref(), Some("kubernetes"));
            assert_eq!(&states(&installed)[..3], [
                ("review".to_string(), ItemState::Installed),
                ("changelog".to_string(), ItemState::Installed),
                ("crashloop".to_string(), ItemState::Installed),
            ]);
            // Twice is refused, not written over.
            assert_eq!(install_with(BASE, ItemKind::Command, "changelog", &files), Err(CatalogError::Present("/changelog".into())));

            remove(BASE, ItemKind::Skill, "review").unwrap();
            remove(BASE, ItemKind::Runbook, "crashloop").unwrap();
            assert!(!here("skills/review").exists());
            assert!(!here("runbooks/kubernetes/crashloop.md").exists());
            assert!(here("commands/changelog.md").exists());
            let left = catalog_store::load_installed().unwrap();
            assert_eq!(left.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["changelog"]);
            assert_eq!(remove(BASE, ItemKind::Skill, "review"), Err(CatalogError::NotInstalled("review".into())));
        });
    }

    #[test]
    fn an_edit_an_update_and_someone_elses_file_each_read_as_such() {
        with_app_dir("catalog-states", || {
            std::fs::create_dir_all(here("commands")).unwrap();
            std::fs::write(here("commands/changelog.md"), "the user's own").unwrap();
            listed_once(CAT);
            assert_eq!(install_with(BASE, ItemKind::Command, "changelog", &files), Err(CatalogError::Present("/changelog".into())));
            assert_eq!(std::fs::read_to_string(here("commands/changelog.md")).unwrap(), "the user's own");
            assert_eq!(remove(BASE, ItemKind::Command, "changelog"), Err(CatalogError::NotInstalled("changelog".into())));
            assert!(here("commands/changelog.md").exists());

            install_with(BASE, ItemKind::Skill, "review", &files).unwrap();
            let installed = catalog_store::load_installed().unwrap();
            let state = |installed: &[Installed]| states(installed).into_iter().find(|(id, _)| id == "review").unwrap().1;
            assert_eq!(state(&installed), ItemState::Installed);
            assert_eq!(states(&installed)[1], ("changelog".to_string(), ItemState::Taken));

            // The new rev has a file more: what is on disk is still what was written.
            listed_once(&CAT.replace(r#""rev": "1", "path": "skills/review", "files": ["SKILL.md", "refs/list.md"]"#, r#""rev": "2", "path": "skills/review", "files": ["SKILL.md", "refs/list.md", "new.md"]"#));
            assert_eq!(state(&installed), ItemState::Update);
            std::fs::write(here("skills/review/refs/list.md"), "- mine").unwrap();
            assert_eq!(state(&installed), ItemState::Changed);
            // Installed from another catalog: this one does not take it off.
            assert_eq!(remove("https://other.example/", ItemKind::Skill, "review"), Err(CatalogError::NotInstalled("review".into())));
            assert!(here("skills/review").exists());
        });
    }

    #[test]
    fn deleted_by_hand_it_installs_again_under_one_record() {
        with_app_dir("catalog-reinstall", || {
            listed_once(CAT);
            install_with(BASE, ItemKind::Command, "changelog", &files).unwrap();
            std::fs::remove_file(here("commands/changelog.md")).unwrap();
            assert_eq!(states(&catalog_store::load_installed().unwrap())[1].1, ItemState::Available);
            install_with(BASE, ItemKind::Command, "changelog", &files).unwrap();
            assert_eq!(catalog_store::load_installed().unwrap().len(), 1);
        });
    }

    /// Placed but not recorded, it would read as someone else's and never come off from here.
    #[cfg(unix)]
    #[test]
    fn a_record_that_cannot_be_written_takes_the_files_back() {
        use std::os::unix::fs::PermissionsExt;
        with_app_dir("catalog-unrecorded", || {
            listed_once(CAT);
            let record_dir = here("catalog");
            std::fs::set_permissions(&record_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
            let result = install_with(BASE, ItemKind::Skill, "review", &files);
            std::fs::set_permissions(&record_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(matches!(result, Err(CatalogError::Record(_))), "{result:?}");
            assert!(std::fs::symlink_metadata(here("skills/review")).is_err());
        });
    }

    #[test]
    fn a_file_that_fails_to_come_writes_nothing() {
        with_app_dir("catalog-partial", || {
            listed_once(&CAT.replace("refs/list.md", "refs/missing.md"));
            assert_eq!(install_with(BASE, ItemKind::Skill, "review", &files), Err(CatalogError::Status(404)));
            assert!(std::fs::symlink_metadata(here("skills/review")).is_err());
            assert_eq!(catalog_store::load_installed().unwrap(), Vec::new());
        });
    }

    #[test]
    fn only_what_is_listed_installs_and_mcp_waits() {
        with_app_dir("catalog-not-listed", || {
            assert_eq!(install_with(BASE, ItemKind::Command, "changelog", &files), Err(CatalogError::NotListed("changelog".into())));
            listed_once(CAT);
            assert_eq!(install_with(BASE, ItemKind::Command, "other", &files), Err(CatalogError::NotListed("other".into())));
            assert_eq!(install_with(BASE, ItemKind::Skill, "changelog", &files), Err(CatalogError::NotListed("changelog".into())));
            assert_eq!(install_with(BASE, ItemKind::Mcp, "github", &|_, _| panic!("fetched")), Err(CatalogError::NotYet("GitHub".into())));
        });
    }

    #[test]
    fn a_record_that_does_not_read_stops_an_install() {
        with_app_dir("catalog-broken-record", || {
            listed_once(CAT);
            std::fs::create_dir_all(here("catalog")).unwrap();
            std::fs::write(here("catalog/installed.json"), "{").unwrap();
            assert!(matches!(install_with(BASE, ItemKind::Command, "changelog", &files), Err(CatalogError::Record(_))));
            assert!(!here("commands/changelog.md").exists());
        });
    }

    #[test]
    fn a_preview_is_a_skills_skill_md_or_the_one_file() {
        with_app_dir("catalog-preview", || {
            listed_once(CAT);
            assert_eq!(preview_with(BASE, ItemKind::Skill, "review", &files).unwrap(), "# Review");
            assert_eq!(preview_with(BASE, ItemKind::Command, "changelog", &files).unwrap(), "Add a line");
        });
    }

    /// The app's own catalog: every entry that installs as files installed
    /// into a throwaway app directory, read back as installed, and removed —
    /// ignored: it needs the network. Run after touching install or the
    /// catalog: `cargo test live_catalog -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_catalog_installs_and_removes() {
        with_app_dir("catalog-live-install", || {
            let source = catalog::DEFAULT_SOURCE;
            let listed = read(source, &catalog_http::fetch_index, 1, &[]);
            assert_eq!(listed.error, None);
            let files: Vec<_> = listed.items.iter().filter(|v| v.target.is_some()).map(|v| (v.item.kind, v.item.id.clone())).collect();
            assert!(!files.is_empty());
            for (kind, id) in &files {
                assert!(!preview(source, *kind, id).unwrap().is_empty(), "{id}");
                install(source, *kind, id).unwrap();
            }
            let again = read(source, &catalog_http::fetch_index, 2, &catalog_store::load_installed().unwrap());
            for view in again.items.iter().filter(|v| v.target.is_some()) {
                println!("{} {:?} -> {}", view.item.id, view.state, view.target.as_deref().unwrap_or(""));
                assert_eq!(view.state, ItemState::Installed, "{}", view.item.id);
            }
            for (kind, id) in &files {
                remove(source, *kind, id).unwrap();
            }
            assert_eq!(catalog_store::load_installed().unwrap(), Vec::new());
        });
    }
}
