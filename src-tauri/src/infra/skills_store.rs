//! Skill folders on disk: `<skills dir>/<name>/SKILL.md`, plus whatever files
//! a skill keeps beside it. The skills dirs are the open folder's
//! `.claude/skills` and `.agents/skills`, and the user's: the app's own under
//! the app directory, then `~/.agents/skills` (Codex's) and `~/.claude/skills`
//! (Claude Code's) — where other agents keep skills, so theirs load here
//! unchanged.
//!
//! Ported from Alfa Atlas `infra/user_skills_store.rs`, without import,
//! removal and the Settings preview — those arrive with the skills tab.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::domain::skills::{ParsedSkill, SkillError, parse_skill_md};
use crate::infra::app_dir;

const SKILL_MD: &str = "SKILL.md";

/// How many companion files a loaded skill lists. A skill folder is written
/// by hand and should hold a handful; one that holds a `node_modules` would
/// otherwise put thousands of paths into the model's context.
const MAX_LISTED_FILES: usize = 100;

/// The app's own skills folder — the one an empty tab points to.
pub fn dir() -> Result<PathBuf, SkillError> {
    Ok(app_dir::dir().map_err(|e| SkillError::Io(e.into()))?.join("skills"))
}

/// The user's skills dirs, in the order a name is looked up in them: the
/// app's own first, then the ones other agents install into.
pub fn user_dirs() -> Result<[PathBuf; 3], SkillError> {
    let home = home()?;
    Ok([dir()?, home.join(".agents").join("skills"), home.join(".claude").join("skills")])
}

#[cfg(not(test))]
fn home() -> Result<PathBuf, SkillError> {
    dirs::home_dir().ok_or_else(|| SkillError::Io("no home directory".into()))
}

/// Under test, the throwaway app directory stands in for the home: the
/// developer's own `~/.agents` and `~/.claude` never reach a test.
#[cfg(test)]
fn home() -> Result<PathBuf, SkillError> {
    app_dir::dir().map_err(|e| SkillError::Io(e.into()))
}

/// The open folder's repository: the nearest folder from it up that has a
/// `.git` — a directory, or the file a worktree or a submodule has. `None`
/// outside git.
fn git_root(workspace: &Path) -> Option<&Path> {
    workspace.ancestors().find(|dir| dir.join(".git").exists())
}

/// A repository's skills dirs, in the order a name is looked up in them:
/// `.claude/skills` and `.agents/skills` of the open folder, then of each
/// folder above it up to the repository's root — the nearest first, so a
/// package in a monorepo overrides the root's skill of the same name. As
/// OpenCode does. Outside git only the open folder's: walking up from a folder
/// in the home would reach `~/.claude/skills` and call it the project's.
pub fn project_dirs(workspace: &Path) -> Vec<PathBuf> {
    let top = git_root(workspace).unwrap_or(workspace);
    let mut dirs = Vec::new();
    for dir in workspace.ancestors() {
        dirs.push(dir.join(".claude").join("skills"));
        dirs.push(dir.join(".agents").join("skills"));
        if dir == top {
            break;
        }
    }
    dirs
}

/// What a repository's skills must stay inside: the repository's root, or
/// the open folder outside git.
pub fn project_root(workspace: &Path) -> PathBuf {
    git_root(workspace).unwrap_or(workspace).to_path_buf()
}

/// Each folder under the skills directory, parsed or not. A folder whose
/// `SKILL.md` is broken is still an entry: the caller decides whether it
/// is shown (a settings list should say what is wrong) or skipped (the
/// model's catalog must not offer it).
pub struct SkillEntry {
    pub dir_name: String,
    /// The skill's own folder — what `load` and `read` take.
    pub root: PathBuf,
    pub parsed: Result<ParsedSkill, SkillError>,
}

/// A missing directory is no skills yet, not an error. Sorted by folder name
/// so the catalog the model sees is the same bytes from one turn to the next.
///
/// `within` is the repository a repository's skills must stay inside: their
/// text is sent to the provider, and a link to somewhere else on the disk
/// would send that too. Such a skill is an entry with the reason, like any
/// other that cannot be used.
pub fn scan(dir: &Path, within: Option<&Path>) -> Result<Vec<SkillEntry>, SkillError> {
    let within = match within.map(Path::canonicalize) {
        None => None,
        Some(Ok(root)) => Some(root),
        // The repository is gone: nothing in it to offer.
        Some(Err(_)) => return Ok(Vec::new()),
    };
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(dir, e)),
    };
    let mut entries = Vec::new();
    for entry in read {
        let path = entry.map_err(|e| io(dir, e))?.path();
        let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) else { continue };
        if !path.is_dir() || dir_name.starts_with('.') {
            continue;
        }
        let outside = within.as_ref().is_some_and(|root| !path.canonicalize().is_ok_and(|p| p.starts_with(root)));
        let parsed = if outside {
            Err(SkillError::Io("links outside the repository".into()))
        } else {
            match fs::read_to_string(path.join(SKILL_MD)) {
                Ok(contents) => parse_skill_md(&contents, dir_name),
                Err(e) => Err(io(&path.join(SKILL_MD), e)),
            }
        };
        entries.push(SkillEntry { dir_name: dir_name.to_string(), root: path.clone(), parsed });
    }
    entries.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
    Ok(entries)
}

/// A skill's instructions, and the files beside them it may point to.
///
/// By its folder, as `scan` found it — never a name joined onto a directory,
/// so there is no name that could climb out of one.
pub fn load(root: &Path) -> Result<(ParsedSkill, Vec<String>), SkillError> {
    let name = folder_name(root);
    let contents = fs::read_to_string(root.join(SKILL_MD)).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SkillError::NotFound(name.clone()),
        _ => io(root, e),
    })?;
    let parsed = parse_skill_md(&contents, &name)?;
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    files.truncate(MAX_LISTED_FILES);
    Ok((parsed, files))
}

/// One companion file, by its path inside the skill folder.
pub fn read(root: &Path, relative: &str) -> Result<String, SkillError> {
    let name = folder_name(root);
    if !root.join(SKILL_MD).is_file() {
        return Err(SkillError::NotFound(name));
    }
    let target = resolve_under(root, relative)?;
    fs::read_to_string(&target).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SkillError::Io(format!("{name} has no file {relative}")),
        std::io::ErrorKind::InvalidData => SkillError::Io(format!("{name}/{relative} is not UTF-8 text")),
        _ => io(&target, e),
    })
}

fn folder_name(root: &Path) -> String {
    root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), SkillError> {
    for entry in fs::read_dir(dir).map_err(|e| io(dir, e))? {
        let entry = entry.map_err(|e| io(dir, e))?;
        let path = entry.path();
        if path.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.starts_with('.')) {
            continue;
        }
        // Not `path.is_dir()`: that follows a symlink, and a link to a parent
        // is a walk that never ends.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel != SKILL_MD {
                out.push(rel);
            }
        }
    }
    Ok(())
}

/// `relative` under `root`, refusing anything that leaves it: `..` and
/// absolute paths before the disk is touched, and a symlink pointing out
/// after it resolves.
fn resolve_under(root: &Path, relative: &str) -> Result<PathBuf, SkillError> {
    let escape = || SkillError::PathEscape(relative.to_string());
    let rel = Path::new(relative);
    if relative.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(escape());
    }
    let joined = root.join(rel);
    let Ok(canon) = joined.canonicalize() else {
        // Missing: let the read report it by name.
        return Ok(joined);
    };
    let root = root.canonicalize().map_err(|e| io(root, e))?;
    if canon.starts_with(&root) { Ok(canon) } else { Err(escape()) }
}

fn io(path: &Path, e: std::io::Error) -> SkillError {
    SkillError::Io(format!("{}: {e}", path.display()))
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// Writes `<skills>/<name>/SKILL.md` in the user's folder and returns the skill's folder.
    pub fn write_skill(name: &str, description: &str, body: &str) -> PathBuf {
        write_skill_in(&dir().unwrap(), name, description, body)
    }

    /// Writes `<skills>/<name>/SKILL.md` under `skills` — a repository's, say.
    pub fn write_skill_in(skills: &Path, name: &str, description: &str, body: &str) -> PathBuf {
        let root = skills.join(name);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(SKILL_MD), format!("---\nname: {name}\ndescription: {description}\n---\n{body}")).unwrap();
        root
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{write_skill, write_skill_in};
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};

    fn names(entries: &[SkillEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.dir_name.as_str()).collect()
    }

    #[test]
    fn no_skills_folder_is_no_skills() {
        with_app_dir("skills-none", || assert!(scan(&dir().unwrap(), None).unwrap().is_empty()));
    }

    /// A broken skill is an entry — the tab will want to say what is wrong —
    /// and so is a folder with no SKILL.md; a stray file and a hidden folder are not.
    #[test]
    fn every_folder_is_an_entry_in_name_order() {
        with_app_dir("skills-scan", || {
            let skills = dir().unwrap();
            write_skill("zeta", "Last.", "");
            write_skill("alpha", "First.", "");
            fs::create_dir_all(skills.join("broken")).unwrap();
            fs::write(skills.join("broken").join(SKILL_MD), "no frontmatter").unwrap();
            fs::create_dir_all(skills.join("empty")).unwrap();
            fs::create_dir_all(skills.join(".git")).unwrap();
            fs::write(skills.join("stray.md"), "not a folder").unwrap();

            let entries = scan(&skills, None).unwrap();
            assert_eq!(names(&entries), ["alpha", "broken", "empty", "zeta"]);
            let valid: Vec<&str> = entries.iter().filter(|e| e.parsed.is_ok()).map(|e| e.dir_name.as_str()).collect();
            assert_eq!(valid, ["alpha", "zeta"]);
            assert_eq!(entries[0].root, skills.join("alpha"));
        });
    }

    /// Outside git: the open folder's two, and nothing above it — not the
    /// home's `.claude/skills`, which are the user's.
    #[test]
    fn outside_git_only_the_open_folders_skills_dirs() {
        let ws = temp_dir("skills-dirs-no-git").join("app");
        fs::create_dir_all(&ws).unwrap();
        assert_eq!(project_dirs(&ws), [ws.join(".claude/skills"), ws.join(".agents/skills")]);
        assert_eq!(project_root(&ws), ws);
    }

    /// In a monorepo package: its own dirs, then each folder's above it up to
    /// the repository's root, and not past it.
    #[test]
    fn in_git_every_folder_up_to_the_repositorys_root_nearest_first() {
        let outer = temp_dir("skills-dirs-git");
        let repo = outer.join("repo");
        let package = repo.join("packages/app");
        fs::create_dir_all(&package).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();

        let dirs: Vec<PathBuf> = project_dirs(&package);
        let expected: Vec<PathBuf> = [package.clone(), repo.join("packages"), repo.clone()]
            .iter()
            .flat_map(|d| [d.join(".claude/skills"), d.join(".agents/skills")])
            .collect();
        assert_eq!(dirs, expected);
        assert_eq!(project_root(&package), repo);
    }

    /// A worktree or a submodule has a `.git` file, not a directory: it is a
    /// repository's root all the same.
    #[test]
    fn a_git_file_is_a_repositorys_root_too() {
        let outer = temp_dir("skills-dirs-git-file");
        fs::create_dir_all(outer.join(".git")).unwrap();
        let module = outer.join("vendor/lib");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join(".git"), "gitdir: ../../.git/modules/lib").unwrap();

        assert_eq!(project_dirs(&module), [module.join(".claude/skills"), module.join(".agents/skills")]);
        assert_eq!(project_root(&module), module);
    }

    #[test]
    fn the_users_skills_dirs_are_the_apps_then_codexs_then_claude_codes() {
        with_app_dir("skills-user-dirs", || {
            let home = app_dir::dir().unwrap();
            assert_eq!(user_dirs().unwrap(), [dir().unwrap(), home.join(".agents/skills"), home.join(".claude/skills")]);
        });
    }

    #[test]
    fn a_skill_loads_with_its_files_and_reads_one_of_them() {
        with_app_dir("skills-load", || {
            let root = write_skill("my-skill", "Does a thing.", "# Steps\n");
            fs::create_dir_all(root.join("references")).unwrap();
            fs::write(root.join("references/notes.md"), "extra").unwrap();
            fs::write(root.join(".DS_Store"), "junk").unwrap();

            let (parsed, files) = load(&root).unwrap();
            assert_eq!(parsed.body, "# Steps\n");
            assert_eq!(files, ["references/notes.md"]);
            assert_eq!(read(&root, "references/notes.md").unwrap(), "extra");
        });
    }

    #[test]
    fn a_folder_without_a_skill_is_not_found() {
        with_app_dir("skills-unknown", || {
            let gone = dir().unwrap().join("nope");
            assert_eq!(load(&gone).unwrap_err(), SkillError::NotFound("nope".into()));
            assert_eq!(read(&gone, "x.md").unwrap_err(), SkillError::NotFound("nope".into()));
        });
    }

    #[test]
    fn a_path_outside_the_skill_folder_is_refused() {
        with_app_dir("skills-escape", || {
            let root = write_skill("my-skill", "Does a thing.", "");
            write_skill("other", "Another.", "");
            for path in ["../other/SKILL.md", "/etc/hosts", "", "a/../../other/SKILL.md"] {
                assert!(matches!(read(&root, path), Err(SkillError::PathEscape(_))), "{path:?} was not refused");
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_skill_folder_is_refused() {
        with_app_dir("skills-symlink", || {
            let root = write_skill("my-skill", "Does a thing.", "");
            let outside = temp_dir("skills-outside").join("secret.txt");
            fs::write(&outside, "secret").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
            assert!(matches!(read(&root, "link.txt"), Err(SkillError::PathEscape(_))));
        });
    }

    /// A repository's skill is sent to the provider like its AGENTS.md: one
    /// that links to a folder elsewhere on the disk would send that folder.
    #[cfg(unix)]
    #[test]
    fn a_repositorys_skill_that_links_out_of_it_is_listed_with_the_reason() {
        let ws = temp_dir("skills-project-link");
        let skills = project_dirs(&ws)[0].clone();
        write_skill_in(&skills, "inside", "Stays.", "");
        let elsewhere = temp_dir("skills-project-elsewhere");
        write_skill_in(&elsewhere, "outside", "Leaves.", "");
        std::os::unix::fs::symlink(elsewhere.join("outside"), skills.join("outside")).unwrap();

        let entries = scan(&skills, Some(&ws)).unwrap();
        assert_eq!(names(&entries), ["inside", "outside"]);
        assert!(entries[0].parsed.is_ok());
        assert_eq!(entries[1].parsed.as_ref().unwrap_err(), &SkillError::Io("links outside the repository".into()));
        // The user's own folder has no such bound.
        assert!(scan(&skills, None).unwrap()[1].parsed.is_ok());
    }

    #[test]
    fn an_open_folder_that_is_gone_has_no_skills() {
        let ws = temp_dir("skills-project-gone");
        write_skill_in(&project_dirs(&ws)[0], "release", "Cuts a release.", "");
        let skills = project_dirs(&ws)[0].clone();
        assert!(scan(&skills, Some(&ws.join("missing"))).unwrap().is_empty());
    }
}
