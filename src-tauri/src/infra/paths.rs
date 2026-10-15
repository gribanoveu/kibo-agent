//! Paths in the form they may leave this process in.

use std::path::{Path, PathBuf};

/// `canonicalize` in the form a path may leave this process in: on Windows
/// without the `\\?\` prefix — which libgit2 rejects, and which no plain path
/// starts with — wherever the path stays valid without it. One too long for
/// that, or with a reserved name (`CON`), keeps it rather than being corrupted.
pub fn canonicalize_plain(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}
