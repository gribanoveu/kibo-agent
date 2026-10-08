//! Finding the open folder's instruction files, and which the user switched
//! off.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::project_rules::{MEMORY_NAME, RULE_FILES, RuleFile, RuleListItem};
use crate::domain::settings::SettingsError;
use crate::infra::{app_dir, settings_store};

/// A rule file found at the root: where it really is, and its text or why
/// there is none.
struct Found {
    name: &'static str,
    path: PathBuf,
    text: Result<String, String>,
}

fn find(root: &Path) -> Vec<Found> {
    let Ok(root) = root.canonicalize() else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for &name in RULE_FILES {
        let Ok(path) = root.join(name).canonicalize() else { continue };
        // `CLAUDE.md -> AGENTS.md` is one file, and is sent once.
        if !seen.insert(path.clone()) {
            continue;
        }
        // The text is sent to the provider on every request, so a link to
        // somewhere else on the disk would send that too.
        let text = if !path.starts_with(&root) {
            Err("links outside the open folder".to_string())
        } else {
            read(&path)
        };
        found.push(Found { name, path, text });
    }
    // Last, and only once something was saved: an empty memory is not a row.
    if let Some(path) = memory_path(&root).filter(|path| path.exists()) {
        let text = read(&path);
        found.push(Found { name: MEMORY_NAME, path, text });
    }
    found
}

fn read(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err("not a file".to_string());
    }
    fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => "not UTF-8 text".to_string(),
        _ => e.to_string(),
    })
}

/// Where the agent's notes about `root` are kept: a file per folder under the
/// app directory, named by a hash of the folder's real path. `None` when the
/// folder or the app directory cannot be resolved.
pub fn memory_path(root: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().ok()?;
    let hash = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    Some(app_dir::dir().ok()?.join("memory").join(format!("{}.md", &hash[..16])))
}

/// What the turn's prompt carries: every readable file that is switched on.
///
/// Unreadable settings read as "nothing switched off": the turn goes ahead,
/// and the settings store already refuses to overwrite a file it could not
/// parse.
pub fn load(root: &Path) -> Vec<RuleFile> {
    let settings = settings_store::load().unwrap_or_default().rules;
    find(root)
        .into_iter()
        .filter(|f| settings.is_enabled(&key(&f.path)))
        .filter_map(|f| f.text.ok().map(|text| RuleFile::new(f.name, &text)))
        .collect()
}

/// The rules tab: every file found, including the ones that cannot be used.
pub fn list(root: &Path) -> Vec<RuleListItem> {
    let settings = settings_store::load().unwrap_or_default().rules;
    find(root)
        .into_iter()
        .map(|f| {
            let path = key(&f.path);
            match f.text {
                Ok(text) => {
                    let rule = RuleFile::new(f.name, &text);
                    RuleListItem {
                        enabled: settings.is_enabled(&path),
                        name: rule.name,
                        path,
                        content: rule.content,
                        truncated: rule.truncated,
                        error: None,
                    }
                }
                Err(error) => RuleListItem {
                    name: f.name.to_string(),
                    path,
                    enabled: false,
                    content: String::new(),
                    truncated: false,
                    error: Some(error),
                },
            }
        })
        .collect()
}

/// Refuses on settings it cannot read, rather than saving the defaults over
/// the user's whole configuration.
pub fn set_enabled(path: &str, enabled: bool) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.rules.set_enabled(path, enabled);
    settings_store::save(&settings)
}

fn key(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};

    fn names(rules: &[RuleFile]) -> Vec<&str> {
        rules.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn both_files_are_read_in_order_and_nothing_else() {
        with_app_dir("rules-both", || {
            let root = temp_dir("rules-both-repo");
            fs::write(root.join("CLAUDE.md"), "claude").unwrap();
            fs::write(root.join("AGENTS.md"), "agents").unwrap();
            fs::write(root.join("README.md"), "readme").unwrap();

            let rules = load(&root);
            assert_eq!(names(&rules), ["AGENTS.md", "CLAUDE.md"]);
            assert_eq!(rules[0].content, "agents");
        });
    }

    #[test]
    fn a_folder_without_them_has_no_rules() {
        with_app_dir("rules-none", || {
            assert!(load(&temp_dir("rules-none-repo")).is_empty());
            assert!(load(Path::new("/no/such/folder")).is_empty());
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_the_other_file_is_read_once() {
        with_app_dir("rules-link", || {
            let root = temp_dir("rules-link-repo");
            fs::write(root.join("AGENTS.md"), "agents").unwrap();
            std::os::unix::fs::symlink("AGENTS.md", root.join("CLAUDE.md")).unwrap();

            assert_eq!(names(&load(&root)), ["AGENTS.md"]);
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_folder_is_listed_with_why_and_never_sent() {
        with_app_dir("rules-escape", || {
            let root = temp_dir("rules-escape-repo");
            let secret = temp_dir("rules-escape-outside").join("notes.md");
            fs::write(&secret, "private").unwrap();
            std::os::unix::fs::symlink(&secret, root.join("AGENTS.md")).unwrap();

            assert!(load(&root).is_empty());
            let listed = list(&root);
            assert_eq!(listed[0].error.as_deref(), Some("links outside the open folder"));
            assert_eq!(listed[0].content, "");
            assert!(!listed[0].enabled);
        });
    }

    #[test]
    fn a_file_that_is_not_text_is_listed_with_why() {
        with_app_dir("rules-binary", || {
            let root = temp_dir("rules-binary-repo");
            fs::write(root.join("AGENTS.md"), [0xff, 0xfe, 0x00]).unwrap();
            assert!(load(&root).is_empty());
            assert_eq!(list(&root)[0].error.as_deref(), Some("not UTF-8 text"));
        });
    }

    /// Per file, so switching off one repository's `AGENTS.md` leaves the
    /// next repository's on.
    #[test]
    fn a_switched_off_file_leaves_the_prompt_but_not_the_list() {
        with_app_dir("rules-toggle", || {
            let root = temp_dir("rules-toggle-repo");
            let other = temp_dir("rules-toggle-other");
            fs::write(root.join("AGENTS.md"), "agents").unwrap();
            fs::write(root.join("CLAUDE.md"), "claude").unwrap();
            fs::write(other.join("AGENTS.md"), "other").unwrap();

            let listed = list(&root);
            assert!(listed.iter().all(|r| r.enabled));
            set_enabled(&listed[0].path, false).unwrap();

            assert_eq!(names(&load(&root)), ["CLAUDE.md"]);
            assert!(!list(&root)[0].enabled);
            assert_eq!(names(&load(&other)), ["AGENTS.md"]);

            set_enabled(&listed[0].path, true).unwrap();
            assert_eq!(names(&load(&root)), ["AGENTS.md", "CLAUDE.md"]);
        });
    }

    #[test]
    fn unreadable_settings_refuse_the_switch_rather_than_overwrite_them() {
        with_app_dir("rules-damaged", || {
            crate::infra::app_dir::ensure().unwrap();
            fs::write(settings_store::path().unwrap(), "{ not json").unwrap();
            assert!(set_enabled("/repo/AGENTS.md", false).is_err());
            assert_eq!(fs::read_to_string(settings_store::path().unwrap()).unwrap(), "{ not json");
        });
    }
}
