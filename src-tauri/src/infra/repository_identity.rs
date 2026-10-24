//! Which repository an index belongs to.
//!
//! The index does not live in the repository. It lives in the app's own
//! directory, under a folder named after the repository, so a working tree
//! stays clean of a database nobody asked for and so the work survives a
//! `git clean`. That needs a name for the repository — this module.
//!
//! **Identity is the remote, not the revision.** Switching branches, pulling,
//! committing: none of it changes which folder a repository maps to. That is
//! the whole point — a branch switch that reset the index would make the
//! feature useless on the one workflow it exists for. What changes with the
//! content is the per-chunk hash, one layer up.
//!
//! Two clones of the same repository share a folder, deliberately: the chunks
//! are keyed by content hash, so whichever clone indexes a file first saves the
//! other the work.

use std::path::Path;

use git2::Repository;

/// A stable name for the repository at `root`, as 64 hex characters.
///
/// Never fails. A directory that is not a repository, or one with no remote,
/// is an ordinary state rather than a fault — most work starts that way — and
/// it is named after its own canonical path instead.
///
/// The path fallback trades one thing: moving or renaming the folder gives it
/// a new identity and the index is built again. Upstream avoids that by
/// writing a generated id into a file inside the project, which is a file in
/// somebody's working tree forever to save a one-time rebuild. Re-indexing a
/// moved folder is the cheaper of the two, and the stale folder is collected
/// with the rest of the app's cache.
pub fn repository_id(root: &Path) -> String {
    let source = remote_identity(root).unwrap_or_else(|| {
        root.canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .into_owned()
    });
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

/// A name for people and for document ids: the last part of the remote
/// (`git@host:org/tax-api.git` → `tax-api`), or the folder's own name without
/// one. Lowercase, `[a-z0-9._-]`, never empty. Not an identity: two
/// repositories can share it, and the knowledge base tells them apart by
/// [`repository_id`].
pub fn repository_name(root: &Path) -> String {
    let source = remote_identity(root).unwrap_or_else(|| {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    });
    let last = source.rsplit('/').next().unwrap_or_default();
    let name: String = last
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    let name = name.trim_matches(['-', '.']);
    if name.is_empty() { "repository".to_string() } else { name.to_string() }
}

/// The commit checked out at `root`; `None` outside a repository or before
/// its first commit.
pub fn head_commit(root: &Path) -> Option<String> {
    let repo = Repository::open(root).ok()?;
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

/// The canonical form of this repository's remote, if it has exactly one
/// obvious candidate.
///
/// `origin` wins when it exists. Failing that, a single remote is
/// unambiguous. Several remotes with no `origin` among them is a question
/// this module has no business guessing at, so it declines and the caller
/// falls back to the path — a private index rather than one silently shared
/// with whichever remote happened to sort first.
fn remote_identity(root: &Path) -> Option<String> {
    let repo = Repository::open(root).ok()?;
    let remotes = repo.remotes().ok()?;
    let names: Vec<&str> = remotes.iter().filter_map(|name| name.ok().flatten()).collect();

    let name = if names.contains(&"origin") {
        "origin"
    } else {
        match names.as_slice() {
            [only] => only,
            _ => return None,
        }
    };

    let remote = repo.find_remote(name).ok()?;
    Some(canonicalize_remote_url(remote.url().ok()?))
}

/// Collapses the ways of writing one remote into a single string, so that
/// `git@github.com:org/repo.git`, `ssh://git@github.com/org/repo.git` and
/// `https://github.com/org/repo` are all the same repository.
///
/// Drops the scheme, any embedded credentials, a trailing `.git` and a
/// trailing slash. Lowercases the host and nothing else: host names are never
/// case-sensitive and paths on some hosts are.
fn canonicalize_remote_url(url: &str) -> String {
    let trimmed = url.trim();

    // `user@host:path` with no scheme — git's own shorthand. Rewritten into
    // the `host/path` shape the other forms reduce to below.
    let normalized = if !trimmed.contains("://") && trimmed.contains(':') && trimmed.contains('@') {
        let after_at = trimmed.split_once('@').map_or(trimmed, |(_, rest)| rest);
        after_at.replacen(':', "/", 1)
    } else if let Some(at) = trimmed.find("://") {
        trimmed[at + 3..].to_string()
    } else {
        trimmed.to_string()
    };

    // Credentials left over from a scheme-prefixed form. A token in a remote
    // URL is a real thing people have, and hashing it into a folder name would
    // give the same repository two identities on two machines — besides
    // putting a secret somewhere nobody expects one.
    let without_credentials = match normalized.split_once('@') {
        Some((_, rest)) => rest,
        None => normalized.as_str(),
    };

    let trimmed_path = without_credentials.trim_end_matches('/');
    let bare = trimmed_path.strip_suffix(".git").unwrap_or(trimmed_path);

    match bare.split_once('/') {
        Some((host, path)) => format!("{}/{}", host.to_lowercase(), path),
        None => bare.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::path::PathBuf;

    fn repo_with_remotes(label: &str, remotes: &[(&str, &str)]) -> PathBuf {
        let dir = temp_dir(label);
        let repo = Repository::init(&dir).expect("a repository");
        for (name, url) in remotes {
            repo.remote(name, url).expect("a remote");
        }
        dir
    }

    // ------------------------------------------------------ the remote's name

    #[test]
    fn the_ways_of_writing_one_remote_collapse_to_one_name() {
        let https = canonicalize_remote_url("https://github.com/org/repo.git");

        for other in [
            "ssh://git@github.com/org/repo.git",
            "git@github.com:org/repo.git",
            "https://github.com/org/repo/",
            "https://GitHub.com/org/repo",
            "  https://github.com/org/repo  ",
        ] {
            assert_eq!(canonicalize_remote_url(other), https, "{other}");
        }
        assert_eq!(https, "github.com/org/repo");
    }

    /// A token in a remote URL is ordinary. Hashed into the identity it would
    /// give one repository two names on two machines — and put a secret into a
    /// folder name.
    #[test]
    fn credentials_in_the_url_are_not_part_of_the_identity() {
        assert_eq!(
            canonicalize_remote_url("https://user:ghp_secret@github.com/org/repo.git"),
            canonicalize_remote_url("https://github.com/org/repo.git")
        );
    }

    /// Host names are case-insensitive; paths on some hosts are not.
    /// Lowercasing everything would merge two different repositories.
    #[test]
    fn only_the_host_is_lowercased() {
        assert_eq!(
            canonicalize_remote_url("https://GitHub.com/Org/Repo"),
            "github.com/Org/Repo"
        );
    }

    // ------------------------------------------------------- the readable name

    #[test]
    fn the_name_is_the_remotes_last_part_or_the_folders() {
        let ssh = repo_with_remotes("name-ssh", &[("origin", "git@github.com:Org/Tax-API.git")]);
        assert_eq!(repository_name(&ssh), "tax-api");
        let https = repo_with_remotes("name-https", &[("origin", "https://gitlab.example.com/a/b/usn_service/")]);
        assert_eq!(repository_name(&https), "usn_service");

        let folder = temp_dir("Plain.Folder");
        assert!(repository_name(&folder).starts_with("plain.folder-"), "{}", repository_name(&folder));
        // `temp_dir` adds `-<n>`: what is left of «Мой проект-<n>» is the number.
        let cyrillic = repository_name(&temp_dir("Мой проект"));
        assert!(!cyrillic.is_empty() && cyrillic.chars().all(|c| c.is_ascii_digit()), "{cyrillic}");
        let nothing_left = temp_dir("name-none").join("Проект");
        std::fs::create_dir(&nothing_left).unwrap();
        assert_eq!(repository_name(&nothing_left), "repository");
    }

    #[test]
    fn the_head_is_the_checked_out_commit() {
        let dir = repo_with_remotes("head-commit", &[]);
        assert_eq!(head_commit(&dir), None, "no commit yet");
        let repo = Repository::open(&dir).unwrap();
        let tree = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree).unwrap();
        let me = git2::Signature::now("t", "t@example.com").unwrap();
        let id = repo.commit(Some("HEAD"), &me, &me, "first", &tree, &[]).unwrap();
        assert_eq!(head_commit(&dir), Some(id.to_string()));
        assert_eq!(head_commit(&temp_dir("head-plain")), None);
    }

    // -------------------------------------------------------------- identity

    #[test]
    fn the_id_is_stable_and_distinct() {
        let repo = repo_with_remotes("identity-stable", &[("origin", "git@github.com:org/repo.git")]);
        let other = repo_with_remotes("identity-other", &[("origin", "git@github.com:org/other.git")]);

        assert_eq!(repository_id(&repo), repository_id(&repo));
        assert_ne!(repository_id(&repo), repository_id(&other));
        assert_eq!(repository_id(&repo).len(), 64);
        assert!(repository_id(&repo).chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The reason identity is the remote and not the path: two clones of one
    /// repository index it once between them.
    #[test]
    fn two_clones_of_one_repository_share_an_identity() {
        let here = repo_with_remotes("identity-clone-a", &[("origin", "git@github.com:org/repo.git")]);
        let there =
            repo_with_remotes("identity-clone-b", &[("origin", "https://github.com/org/repo")]);

        assert_eq!(repository_id(&here), repository_id(&there));
    }

    /// Most work starts in a directory that is not a repository yet. It gets
    /// an identity anyway, or the index has nowhere to go.
    #[test]
    fn a_directory_that_is_not_a_repository_still_gets_a_name() {
        let plain = temp_dir("identity-plain");
        let id = repository_id(&plain);

        assert_eq!(id.len(), 64);
        assert_eq!(id, repository_id(&plain), "the same folder, twice");
        assert_ne!(id, repository_id(&temp_dir("identity-plain-other")));
    }

    #[test]
    fn a_repository_with_no_remote_falls_back_to_its_path() {
        let local = repo_with_remotes("identity-no-remote", &[]);
        let plain_copy = temp_dir("identity-no-remote-elsewhere");

        assert_eq!(repository_id(&local).len(), 64);
        assert_ne!(repository_id(&local), repository_id(&plain_copy));
    }

    /// A fork: `origin` is mine, `upstream` is theirs, and the index belongs
    /// to mine. Picking by sort order would put it under theirs.
    #[test]
    fn origin_wins_over_any_other_remote() {
        let fork = repo_with_remotes(
            "identity-fork",
            &[
                ("aaa-upstream", "git@github.com:them/repo.git"),
                ("origin", "git@github.com:me/repo.git"),
            ],
        );
        let mine = repo_with_remotes("identity-mine", &[("origin", "git@github.com:me/repo.git")]);

        assert_eq!(repository_id(&fork), repository_id(&mine));
    }

    /// Several remotes and no `origin` is a question with no right answer.
    /// Guessing would share an index with a repository the user never named;
    /// falling back to the path keeps it private.
    #[test]
    fn several_remotes_without_an_origin_fall_back_to_the_path() {
        let ambiguous = repo_with_remotes(
            "identity-ambiguous",
            &[
                ("fork", "git@github.com:me/repo.git"),
                ("shared", "git@github.com:them/repo.git"),
            ],
        );
        let named = repo_with_remotes("identity-named", &[("origin", "git@github.com:me/repo.git")]);

        assert_ne!(repository_id(&ambiguous), repository_id(&named));
    }

    /// Committing, branching, pulling — none of it may move the index. A
    /// branch switch that reset it would break the one workflow this exists
    /// for.
    #[test]
    fn a_commit_does_not_change_the_identity() {
        let dir = repo_with_remotes("identity-commit", &[("origin", "git@github.com:org/repo.git")]);
        let before = repository_id(&dir);

        let repo = Repository::open(&dir).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap();

        assert_eq!(repository_id(&dir), before);
    }
}
