//! Command files on disk: `*.md` in the open folder's `.kibo/commands` and in
//! the user's, `~/.kibo/commands` (under the app directory). Only these two —
//! `.claude/commands` and `.opencode/command` are other agents' folders.
//!
//! Read whole each time the list is asked for: a handful of small files, and
//! asked for when the menu opens, so a command just written is there to pick.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::slash_commands::{self, CommandFile, CommandSource};
use crate::infra::{app_dir, skills_store};

/// Past this a file is not a prompt someone wrote by hand, and all of it would
/// be sent as one message.
const MAX_FILE_BYTES: u64 = 64 * 1024;

pub fn commands_dir(root: &Path) -> PathBuf {
    root.join(".kibo").join("commands")
}

/// The open folder's commands and the user's, the folder's first on a shared
/// name. Only the user's while no folder is open.
pub fn list(workspace: Option<&Path>) -> Vec<CommandFile> {
    let project = workspace
        .map(|ws| scan(&commands_dir(ws), Some(&skills_store::project_root(ws)), CommandSource::Project))
        .unwrap_or_default();
    let user = app_dir::dir()
        .map(|dir| scan(&dir.join("commands"), None, CommandSource::User))
        .unwrap_or_default();
    slash_commands::merge(project, user)
}

/// Every command in one folder. A missing folder is no commands; a file that
/// cannot be read, is too large, or links outside `within` (its text goes to
/// the provider) is left out rather than failing the rest.
fn scan(dir: &Path, within: Option<&Path>, source: CommandSource) -> Vec<CommandFile> {
    let within = within.and_then(|root| root.canonicalize().ok());
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    read.filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter_map(|path| {
            let real = path.canonicalize().ok()?;
            if within.as_ref().is_some_and(|root| !real.starts_with(root)) {
                return None;
            }
            let meta = fs::metadata(&real).ok()?;
            if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                return None;
            }
            let stem = path.file_stem()?.to_str()?;
            slash_commands::parse_command_md(stem, &fs::read_to_string(&real).ok()?, source)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};

    fn write(dir: &Path, name: &str, contents: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), contents).unwrap();
    }

    fn names(commands: &[CommandFile]) -> Vec<(&str, CommandSource)> {
        commands.iter().map(|c| (c.name.as_str(), c.source)).collect()
    }

    #[test]
    fn the_folder_s_and_the_user_s_are_read_and_nothing_else() {
        with_app_dir("slash-commands-both", || {
            let ws = temp_dir("slash-commands-ws");
            write(&commands_dir(&ws), "review.md", "Review $ARGUMENTS");
            write(&commands_dir(&ws), "notes.txt", "not a command");
            write(&ws.join(".claude").join("commands"), "claude.md", "someone else's");
            write(&ws.join(".opencode").join("command"), "opencode.md", "someone else's");
            write(&app_dir::dir().unwrap().join("commands"), "changelog.md", "Add a line");

            assert_eq!(
                names(&list(Some(&ws))),
                [("changelog", CommandSource::User), ("review", CommandSource::Project)]
            );
            assert_eq!(names(&list(None)), [("changelog", CommandSource::User)]);
        });
    }

    #[test]
    fn nothing_written_yet_is_no_commands() {
        with_app_dir("slash-commands-none", || {
            assert!(list(Some(&temp_dir("slash-commands-empty"))).is_empty());
        });
    }

    #[test]
    fn a_file_too_large_to_be_a_prompt_is_left_out() {
        let dir = temp_dir("slash-commands-large");
        write(&dir, "huge.md", &"x".repeat(MAX_FILE_BYTES as usize + 1));
        write(&dir, "small.md", "fine");
        assert_eq!(names(&scan(&dir, None, CommandSource::User)), [("small", CommandSource::User)]);
    }

    /// Its text is sent to the provider: a link out of the repository would
    /// send whatever it points at.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_repository_is_left_out() {
        let ws = temp_dir("slash-commands-link-ws");
        let outside = temp_dir("slash-commands-link-out");
        write(&outside, "secret.md", "the contents of somewhere else");
        write(&commands_dir(&ws), "own.md", "fine");
        std::os::unix::fs::symlink(outside.join("secret.md"), commands_dir(&ws).join("secret.md")).unwrap();

        let found = scan(&commands_dir(&ws), Some(&ws), CommandSource::Project);
        assert_eq!(names(&found), [("own", CommandSource::Project)]);
    }
}
