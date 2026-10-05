//! Whether the open folder's own skills and `/` commands are read: only once
//! the user trusts it (`domain::settings::FolderTrust`). The folder asked about
//! is the repository's root — what those skills must stay inside — or the open
//! folder outside git.
//!
//! Its `AGENTS.md` and `CLAUDE.md` are read either way, as pi reads them: a
//! repository's instructions are what working in it means.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::domain::settings::SettingsError;
use crate::infra::{settings_store, skills_store, slash_commands_store};

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderTrustView {
    /// The folder the answer is for.
    pub folder: PathBuf,
    /// Whether it has skills or commands of its own: a folder with nothing
    /// trust would let in is not asked about.
    pub needed: bool,
    /// The answer for it or a folder above it; `None` — never asked.
    pub trusted: Option<bool>,
}

pub fn view(workspace: &Path) -> FolderTrustView {
    let folder = skills_store::project_root(workspace);
    let trusted = settings_store::load().unwrap_or_default().folder_trust.decision(&folder);
    FolderTrustView { needed: has_its_own(workspace), folder, trusted }
}

/// Fails on settings it cannot read rather than overwrite them.
pub fn set(workspace: &Path, trusted: bool) -> Result<(), SettingsError> {
    let mut settings = settings_store::load()?;
    settings.folder_trust.set(&skills_store::project_root(workspace), trusted);
    settings_store::save(&settings)
}

/// The open folder when its own skills and commands may be read, `None` when
/// they may not — what their lists take for a folder. Settings that cannot be
/// read trust nothing.
pub fn readable(workspace: Option<&Path>) -> Option<&Path> {
    let trust = settings_store::load().unwrap_or_default().folder_trust;
    workspace.filter(|ws| trust.decision(&skills_store::project_root(ws)) == Some(true))
}

/// Anything in a skills folder of the repository, or in its `.kibo/commands`.
fn has_its_own(workspace: &Path) -> bool {
    skills_store::project_dirs(workspace)
        .into_iter()
        .chain([slash_commands_store::commands_dir(workspace)])
        .any(|dir| fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};

    #[test]
    fn a_folder_with_nothing_of_its_own_is_not_asked_about() {
        with_app_dir("trust-empty", || {
            let ws = temp_dir("trust-empty-ws");
            fs::create_dir_all(ws.join(".kibo").join("commands")).unwrap();
            assert_eq!(view(&ws), FolderTrustView { folder: ws.clone(), needed: false, trusted: None });
        });
    }

    #[test]
    fn skills_or_commands_of_its_own_ask_and_the_answer_is_kept_for_the_repository() {
        with_app_dir("trust-ask", || {
            let ws = temp_dir("trust-ask-ws");
            fs::create_dir(ws.join(".git")).unwrap();
            let package = ws.join("packages").join("web");
            fs::create_dir_all(package.join(".kibo").join("commands")).unwrap();
            fs::write(package.join(".kibo").join("commands").join("ship.md"), "Ship it.").unwrap();
            assert!(view(&package).needed, "commands");
            assert_eq!(readable(Some(&package)), None, "never asked");

            set(&package, true).unwrap();
            let seen = view(&package);
            assert_eq!((seen.folder, seen.trusted), (ws.clone(), Some(true)), "the repository's root is the one answered");
            assert_eq!(readable(Some(&package)), Some(package.as_path()));
            assert_eq!(readable(None), None);

            set(&package, false).unwrap();
            assert_eq!(readable(Some(&package)), None);

            let skills = temp_dir("trust-ask-skills");
            fs::create_dir_all(skills_store::project_dirs(&skills)[1].join("lint")).unwrap();
            assert!(view(&skills).needed, "skills");
        });
    }
}
