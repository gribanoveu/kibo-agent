//! Lists one folder of the workspace for the Files tab: the gitignore-aware
//! walk every tool uses, one level deep, with git's view of each entry.

use std::collections::HashMap;
use std::path::{Component, Path};

use git2::{Repository, Status, StatusOptions};

use crate::domain::file_tree::{FileStatus, FileTreeError, FolderListing, TreeEntry, MAX_ENTRIES};
use crate::domain::tools::canonicalize_plain;
use crate::infra::workspace_scanner::scan_entries;

/// `dir` is relative to `root`; empty is `root` itself.
pub fn list(root: &Path, dir: &str) -> Result<FolderListing, FileTreeError> {
    let rel = Path::new(dir);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(FileTreeError::InvalidPath(dir.to_string()));
    }
    let root = canonicalize_plain(root).map_err(|e| FileTreeError::Io(root.display().to_string(), e.to_string()))?;
    let folder = root.join(rel);
    if !folder.is_dir() {
        return Err(FileTreeError::InvalidPath(dir.to_string()));
    }
    let scanned = scan_entries(&folder, Some(1)).map_err(|e| FileTreeError::Io(dir.to_string(), e.to_string()))?;
    let changes = git_changes(&root, dir);

    let mut entries: Vec<TreeEntry> = scanned
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
            let name = entry.path.file_name()?.to_string_lossy().into_owned();
            let status = if entry.is_dir { None } else { changes.get(&path).copied() };
            let under = format!("{path}/");
            // A file has nothing under it; the check skips a scan per file.
            let changed = entry.is_dir && changes.keys().any(|changed| changed.starts_with(&under));
            Some(TreeEntry { name, path, is_dir: entry.is_dir, status, changed })
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let more = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    Ok(FolderListing { entries, more })
}

/// Changed files under `dir`, by their path relative to `root`. Empty outside
/// a repository: an ordinary folder lists the same, unmarked.
fn git_changes(root: &Path, dir: &str) -> HashMap<String, FileStatus> {
    let Ok(repo) = Repository::discover(root) else { return HashMap::new() };
    let Some(workdir) = repo.workdir().and_then(|w| canonicalize_plain(w).ok()) else { return HashMap::new() };
    // The open folder may sit inside the repository: git speaks from its top.
    let Ok(prefix) = root.strip_prefix(&workdir) else { return HashMap::new() };
    let prefix = prefix.to_string_lossy().replace('\\', "/");
    let scope = [prefix.as_str(), dir].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("/");

    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    if !scope.is_empty() {
        options.pathspec(&scope);
    }
    let Ok(statuses) = repo.statuses(Some(&mut options)) else { return HashMap::new() };
    statuses
        .iter()
        .filter_map(|entry| {
            let path = entry.path().ok()?;
            let path = if prefix.is_empty() { path } else { path.strip_prefix(&prefix)?.strip_prefix('/')? };
            Some((path.to_string(), status(entry.status())?))
        })
        .collect()
}

fn status(s: Status) -> Option<FileStatus> {
    if s.contains(Status::CONFLICTED) {
        Some(FileStatus::Conflicted)
    } else if s.contains(Status::WT_NEW) {
        Some(FileStatus::Untracked)
    } else if s.contains(Status::INDEX_NEW) {
        Some(FileStatus::Added)
    } else if s.intersects(
        Status::INDEX_MODIFIED
            | Status::WT_MODIFIED
            | Status::INDEX_RENAMED
            | Status::WT_RENAMED
            | Status::INDEX_TYPECHANGE
            | Status::WT_TYPECHANGE,
    ) {
        Some(FileStatus::Modified)
    } else {
        // Deleted: not on disk, so not in a listing.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::fs;

    fn names(listing: &FolderListing) -> Vec<(&str, bool, Option<FileStatus>, bool)> {
        listing.entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.status, e.changed)).collect()
    }

    fn repo_with_commit() -> std::path::PathBuf {
        let dir = temp_dir("file-tree");
        let repo = Repository::init(&dir).unwrap();
        fs::create_dir_all(dir.join("src/deep")).unwrap();
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::create_dir_all(dir.join("doc")).unwrap();
        fs::write(dir.join("doc/same.md"), "s\n").unwrap();
        fs::write(dir.join("src/a.rs"), "a\n").unwrap();
        fs::write(dir.join("src/deep/b.rs"), "b\n").unwrap();
        fs::write(dir.join("docs/c.md"), "c\n").unwrap();
        fs::write(dir.join("Zeta.txt"), "z\n").unwrap();
        fs::write(dir.join("alpha.txt"), "a\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
        dir
    }

    #[test]
    fn folders_first_then_files_by_name_ignoring_case_one_level() {
        let dir = repo_with_commit();
        let top = list(&dir, "").unwrap();
        assert_eq!(
            names(&top),
            [
                ("doc", true, None, false),
                ("docs", true, None, false),
                ("src", true, None, false),
                ("alpha.txt", false, None, false),
                ("Zeta.txt", false, None, false)
            ]
        );
        assert_eq!(top.entries[2].path, "src");
        let src = list(&dir, "src").unwrap();
        assert_eq!(names(&src), [("deep", true, None, false), ("a.rs", false, None, false)]);
        assert_eq!(src.entries[1].path, "src/a.rs");
    }

    #[test]
    fn a_file_carries_its_status_and_a_folder_whether_anything_under_it_changed() {
        let dir = repo_with_commit();
        fs::write(dir.join("src/deep/b.rs"), "b\nmore\n").unwrap();
        fs::write(dir.join("docs/new.md"), "n\n").unwrap();
        fs::write(dir.join("staged.txt"), "s\n").unwrap();
        let repo = Repository::open(&dir).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged.txt")).unwrap();
        index.write().unwrap();

        let top = list(&dir, "").unwrap();
        let by = |name: &str| top.entries.iter().find(|e| e.name == name).unwrap().clone();
        assert!(by("src").changed && by("docs").changed);
        // A sibling whose name starts the same is not the changed folder.
        assert!(!by("doc").changed, "docs/ changed, not doc/");
        assert_eq!(by("staged.txt").status, Some(FileStatus::Added));
        assert_eq!(by("alpha.txt").status, None);
        let deep = list(&dir, "src/deep").unwrap();
        assert_eq!(deep.entries[0].status, Some(FileStatus::Modified));
        let docs = list(&dir, "docs").unwrap();
        assert_eq!(docs.entries.iter().find(|e| e.name == "new.md").unwrap().status, Some(FileStatus::Untracked));
    }

    #[test]
    fn a_subfolder_of_a_repository_is_listed_from_itself() {
        let dir = repo_with_commit();
        fs::write(dir.join("src/a.rs"), "changed\n").unwrap();
        let src = list(&dir.join("src"), "").unwrap();
        assert_eq!(names(&src), [("deep", true, None, false), ("a.rs", false, Some(FileStatus::Modified), false)]);
        assert_eq!(src.entries[1].path, "a.rs");
    }

    #[test]
    fn ignored_files_are_left_out_and_a_plain_folder_is_listed_unmarked() {
        let dir = repo_with_commit();
        fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
        fs::write(dir.join("noise.log"), "x\n").unwrap();
        assert!(list(&dir, "").unwrap().entries.iter().all(|e| e.name != "noise.log" && e.name != ".git"));

        let plain = temp_dir("file-tree-plain");
        fs::write(plain.join("x.txt"), "x\n").unwrap();
        assert_eq!(names(&list(&plain, "").unwrap()), [("x.txt", false, None, false)]);
    }

    #[test]
    fn refuses_a_path_that_leaves_the_folder_and_caps_a_huge_one() {
        let dir = repo_with_commit();
        for bad in ["..", "../x", "/etc", "./src", "missing"] {
            assert_eq!(list(&dir, bad), Err(FileTreeError::InvalidPath(bad.into())), "{bad}");
        }
        let big = temp_dir("file-tree-big");
        for i in 0..MAX_ENTRIES + 7 {
            fs::write(big.join(format!("f{i}")), "").unwrap();
        }
        let listing = list(&big, "").unwrap();
        assert_eq!((listing.entries.len(), listing.more), (MAX_ENTRIES, 7));
    }
}
