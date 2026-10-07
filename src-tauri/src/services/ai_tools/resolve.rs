//! Turning a path the model wrote into a real path on disk.
//!
//! The enforcement point for CA-3.9: every tool that takes a path comes
//! through here, and nothing else is trusted to have checked. A tool cannot
//! widen its own access by passing an unexpected path — only a `ToolScope`
//! built with a wider root could.
//!
//! Merged from Alfa Atlas `domain/paths.rs` (427 lines) and
//! `services/ai_tools/resolve.rs` (488). Most of both was the two-root world:
//! a path could be spelled relative to the documentation subtree *or* to the
//! repository, so resolution tried aliases, and results had to be translated
//! between the two namespaces. One root deletes all of it — `path_aliases`,
//! `docs_root_prefixes`, `stripped_docs_prefixes`, `docs_rel_to_access_rel`,
//! `to_access_relative`, `access_and_docs_rel`, `resolve_mutable_docs_path`
//! and the `@deps` split are gone. What survives is the containment check and
//! the two traps it walks around, both documented at their fix.
//!
//! This reaches for `std::fs` directly rather than through an infra trait.
//! Containment cannot be decided without resolving symlinks, so the
//! filesystem is not an implementation detail here — it is the thing being
//! asked. A trait would have exactly one implementation and one test double
//! that could not answer the question truthfully anyway.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::domain::tools::canonicalize_plain;

use crate::domain::tools::{ToolError, ToolScope};

/// The path the model named, proven to be inside the scope and to exist.
pub fn resolve_existing(scope: &ToolScope, path: &str) -> Result<PathBuf, ToolError> {
    let resolved = resolve(scope, path)?;
    if !resolved.exists() {
        return Err(ToolError::NotFound(path.to_string()));
    }
    Ok(resolved)
}

/// The path the model named, proven to be inside the scope. The target need
/// not exist yet — this is what write and create destinations resolve through.
pub fn resolve_writable(scope: &ToolScope, path: &str) -> Result<PathBuf, ToolError> {
    resolve(scope, path)
}

/// `/`-separated spelling of `resolved` relative to the scope root — what a
/// tool reports back to the model, so its next call can name the same file.
///
/// Takes no I/O and cannot fail on a well-formed input: `resolved` has to have
/// come from [`resolve_existing`] or [`resolve_writable`], which already
/// canonicalized it and proved it under the root.
pub fn relative_to_root(scope: &ToolScope, resolved: &Path) -> Result<String, ToolError> {
    if resolved == scope.root() {
        return Ok(".".to_string());
    }
    let rel = resolved
        .strip_prefix(scope.root())
        .map_err(|_| ToolError::PathEscape(resolved.display().to_string()))?;

    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(ToolError::PathEscape(resolved.display().to_string())),
        }
    }
    Ok(parts.join("/"))
}

/// Last segment of a `/`-separated relative path, as produced by
/// [`relative_to_root`]. A plain `rsplit` rather than `Path::file_name` — the
/// input is already normalized, and this way no `OsStr` platform quirk applies.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn resolve(scope: &ToolScope, path: &str) -> Result<PathBuf, ToolError> {
    // The prompt names the open folder by its full path, and a model that
    // writes a file's full path back means that file — not root/Users/…,
    // which was "not found" for a file that is there.
    let path = Path::new(path).strip_prefix(scope.root()).ok().and_then(Path::to_str).unwrap_or(path);
    let joined = join_relative(scope.root(), path)?;
    ensure_under(scope.root(), &joined)
}

/// Joins a `/`-separated relative path onto `root`, rejecting anything that
/// could leave it.
///
/// Every segment must be a plain name, which refuses two things at once:
///
/// - `..` — outright, never reinterpreted. Note this is a rule of its own, not
///   a consequence of the containment check below: `a/../a/file.txt` resolves
///   to a file genuinely inside the root, and is still refused.
/// - A component Windows parses as a drive or UNC prefix. `PathBuf::push`
///   **replaces** the whole path when handed one, so an absolute path from a
///   caller silently left the root: `join_relative(r"C:\repo", r"C:\Windows\
///   win.ini")` produced `C:Windows\win.ini`. On Unix the same input stays
///   under the root, so rejecting it also makes the contract identical on
///   every platform instead of quietly stricter on one.
///
/// A leading `/` is not an escape: the empty first segment is skipped, so
/// `/etc/passwd` resolves as `root/etc/passwd`. Reinterpreting rather than
/// rejecting keeps a model that over-qualified a path inside the boundary.
fn join_relative(root: &Path, relative: &str) -> Result<PathBuf, ToolError> {
    if relative.is_empty() || relative == "." {
        return Ok(root.to_path_buf());
    }

    let mut out = root.to_path_buf();
    for part in relative.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        // Only a plain name may be appended. This one line refuses both
        // traversal and drive/UNC prefixes: `..` parses as `Component::ParentDir`
        // and `C:` as `Component::Prefix`, neither of which is `Normal`.
        if Path::new(part).components().next() != Some(Component::Normal(OsStr::new(part))) {
            return Err(ToolError::PathEscape(relative.to_string()));
        }
        out.push(part);
    }
    Ok(out)
}

/// Canonicalizes `path` and proves it is `root` or under it.
///
/// Canonicalizing is the whole point: a symlink inside the root pointing out
/// of it is the escape a string comparison cannot see.
///
/// When the target does not exist yet, this walks up to the nearest ancestor
/// that *does* and rejoins the missing tail onto its canonical form. Not just
/// the immediate parent: writing a file several directories deep into a tree
/// that is about to be created is the ordinary case, and it has to resolve
/// exactly like a one-level-missing target, not fail before the creation gets
/// a chance to run. `path` always descends from `root` — `join_relative` built
/// it — so the walk terminates at `root` at the latest.
fn ensure_under(root: &Path, path: &Path) -> Result<PathBuf, ToolError> {
    let canonical = if path.exists() {
        canonicalize_plain(path).map_err(ToolError::Io)?
    } else {
        let mut existing = path.to_path_buf();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        while !existing.exists() {
            let name = existing
                .file_name()
                .ok_or_else(|| ToolError::PathEscape(path.display().to_string()))?;
            tail.push(name.to_os_string());
            existing = existing
                .parent()
                .ok_or_else(|| ToolError::PathEscape(path.display().to_string()))?
                .to_path_buf();
        }
        let mut canonical = canonicalize_plain(&existing).map_err(ToolError::Io)?;
        for part in tail.into_iter().rev() {
            canonical.push(part);
        }
        canonical
    };

    // Component-wise, not string-wise: `/repo-backup` must not count as being
    // under `/repo`.
    if !canonical.starts_with(root) {
        return Err(ToolError::PathEscape(canonical.display().to_string()));
    }
    Ok(canonical)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    /// A scope over a fresh directory, plus that directory.
    fn scope(label: &str) -> (ToolScope, PathBuf) {
        let dir = temp_dir(label);
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root)
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent is creatable");
        }
        std::fs::write(path, body).expect("file is writable");
    }

    #[test]
    fn resolves_a_path_inside_the_root() {
        let (scope, root) = scope("resolve-plain");
        write(&root.join("src").join("main.rs"), "fn main() {}");

        let found = resolve_existing(&scope, "src/main.rs").expect("resolves");

        assert_eq!(found, root.join("src").join("main.rs"));
        assert_eq!(relative_to_root(&scope, &found).unwrap(), "src/main.rs");
    }

    #[test]
    fn a_missing_file_is_not_found_rather_than_escaping() {
        let (scope, _root) = scope("resolve-missing");
        assert!(matches!(
            resolve_existing(&scope, "src/main.rs"),
            Err(ToolError::NotFound(_))
        ));
    }

    #[test]
    fn parent_traversal_is_refused() {
        let (scope, _root) = scope("resolve-traversal");
        for attempt in ["..", "../secret", "src/../../secret", "a/b/../../../x"] {
            assert!(
                matches!(resolve_writable(&scope, attempt), Err(ToolError::PathEscape(_))),
                "{attempt} was not refused"
            );
        }
    }

    /// `..` is refused as policy, not merely as an outcome. This path never
    /// leaves the root — canonicalizing it lands exactly on the file — so the
    /// containment check would wave it through; only the explicit rejection in
    /// `join_relative` stops it. Pinned separately because the two defences
    /// agree on every escaping input, and a test that only checks escapes
    /// cannot tell which one is still doing the work.
    #[test]
    fn parent_traversal_is_refused_even_when_it_stays_inside() {
        let (scope, root) = scope("resolve-inward-traversal");
        write(&root.join("a").join("file.txt"), "body");

        assert!(
            matches!(
                resolve_existing(&scope, "a/../a/file.txt"),
                Err(ToolError::PathEscape(_))
            ),
            "an inward `..` was allowed — the explicit check is gone"
        );
    }

    /// Not an escape, and deliberately not an error: the segment before the
    /// first `/` is empty, so an over-qualified path folds back under the root
    /// instead of failing. A model that wrote `/src/main.rs` meant `src/main.rs`.
    #[test]
    fn the_roots_own_full_path_names_the_file_under_it() {
        let (scope, root) = scope("resolve-absolute");
        write(&root.join("src").join("main.rs"), "fn main() {}");
        let full = root.join("src").join("main.rs");

        let found = resolve_existing(&scope, full.to_str().unwrap()).expect("resolves");

        assert_eq!(found, full);
        assert_eq!(resolve_existing(&scope, root.to_str().unwrap()).unwrap(), root, "the root itself");
    }

    #[test]
    fn a_leading_slash_folds_back_under_the_root() {
        let (scope, root) = scope("resolve-absolute");
        write(&root.join("etc").join("passwd"), "not the real one");

        let found = resolve_existing(&scope, "/etc/passwd").expect("resolves");

        assert_eq!(found, root.join("etc").join("passwd"));
        assert!(found.starts_with(&root));
    }

    /// The escape a string comparison cannot see, and the entire reason
    /// `ensure_under` canonicalizes instead of concatenating.
    #[cfg(unix)]
    #[test]
    fn a_symlink_leaving_the_root_is_refused() {
        let (scope, root) = scope("resolve-symlink");
        let outside = temp_dir("resolve-symlink-outside");
        write(&outside.join("secret.txt"), "credentials");

        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink is creatable");

        assert!(matches!(
            resolve_existing(&scope, "link/secret.txt"),
            Err(ToolError::PathEscape(_))
        ));
        std::fs::remove_dir_all(&outside).ok();
    }

    /// `/repo-backup` is not inside `/repo`, however much its string says so.
    /// `Path::starts_with` compares components; `str::starts_with` would not.
    #[cfg(unix)]
    #[test]
    fn a_sibling_sharing_the_roots_name_prefix_is_refused() {
        let dir = temp_dir("resolve-sibling");
        let root = dir.join("repo");
        let sibling = dir.join("repo-backup");
        std::fs::create_dir_all(&root).expect("root is creatable");
        write(&sibling.join("secret.txt"), "credentials");

        let scope = ToolScope::new(&root).expect("root resolves");
        std::os::unix::fs::symlink(&sibling, root.join("link")).expect("symlink is creatable");

        assert!(matches!(
            resolve_existing(&scope, "link/secret.txt"),
            Err(ToolError::PathEscape(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Windows turns a drive or UNC component into a `PathBuf::push` that
    /// *replaces* the path. The contract is the same on every platform: this
    /// input never leaves the root — refused where it could escape, folded in
    /// where it could not.
    #[test]
    fn a_drive_qualified_path_never_leaves_the_root() {
        let (scope, root) = scope("resolve-drive");
        for attempt in [r"C:\Windows\win.ini", r"\\server\share\x", "C:/Windows/win.ini"] {
            match resolve_writable(&scope, attempt) {
                Err(ToolError::PathEscape(_)) => {}
                Ok(path) => assert!(path.starts_with(&root), "{attempt} escaped to {path:?}"),
                Err(other) => panic!("{attempt}: unexpected {other}"),
            }
        }
    }

    /// `writeFile` creates missing parents, and a destination several levels
    /// into a tree that does not exist yet has to resolve exactly like a
    /// one-level-missing one — not fail before the creation can run.
    #[test]
    fn a_writable_target_deep_in_a_missing_tree_resolves() {
        let (scope, root) = scope("resolve-deep");

        let target = resolve_writable(&scope, "a/b/c/d.txt").expect("resolves");

        assert_eq!(target, root.join("a").join("b").join("c").join("d.txt"));
        assert_eq!(relative_to_root(&scope, &target).unwrap(), "a/b/c/d.txt");
    }

    #[test]
    fn the_root_itself_is_relative_to_nothing() {
        let (scope, root) = scope("resolve-root");
        assert_eq!(relative_to_root(&scope, &root).unwrap(), ".");
        assert_eq!(resolve_writable(&scope, ".").unwrap(), root);
        assert_eq!(resolve_writable(&scope, "").unwrap(), root);
    }

    #[test]
    fn basename_takes_the_last_segment() {
        assert_eq!(basename("src/main.rs"), "main.rs");
        assert_eq!(basename("main.rs"), "main.rs");
        assert_eq!(basename(""), "");
    }
}
