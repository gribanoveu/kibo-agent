//! `<app dir>/catalog/`: the last index read from each source, with its
//! `ETag` — so the page lists something without the network, and asking
//! again costs a `304` when nothing changed — and `installed.json`, what the
//! catalog put where. Also the writing of an entry into its place, and its
//! removal (`docs/32-catalog.md`).
//!
//! One file per source, named by a hash of its address: the address is a
//! URL, and a URL is not a file name on every platform.

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::catalog::{content_hash, Installed};
use crate::infra::app_dir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedIndex {
    /// The address it was read from: a hash names the file, this says whose it is.
    pub source: String,
    pub etag: Option<String>,
    /// Unix seconds of the last answer, `200` or `304`.
    pub fetched_at: u64,
    /// The index as it came, parsed again on reading — the parser of the app
    /// reading it, not of the one that wrote it.
    pub text: String,
}

fn path(source: &str) -> Result<PathBuf, String> {
    let name = blake3::hash(source.as_bytes()).to_hex();
    Ok(app_dir::dir()?.join("catalog").join(format!("{}.json", &name[..16])))
}

/// `None` when nothing was kept, or what was kept no longer reads — either
/// way the source is asked as if for the first time.
pub fn load(source: &str) -> Option<CachedIndex> {
    let text = fs::read_to_string(path(source).ok()?).ok()?;
    serde_json::from_str::<CachedIndex>(&text).ok().filter(|cached| cached.source == source)
}

pub fn save(cached: &CachedIndex) -> Result<(), String> {
    let text = serde_json::to_string(cached).map_err(|e| e.to_string())?;
    app_dir::write_private(&path(&cached.source)?, text.as_bytes())
}

fn installed_path() -> Result<PathBuf, String> {
    Ok(app_dir::dir()?.join("catalog").join("installed.json"))
}

#[derive(Default, Serialize, Deserialize)]
struct InstalledFile {
    items: Vec<Installed>,
}

/// Nothing installed yet is an empty list; a file that no longer reads is an
/// error — taken for empty, the next install would write over every record,
/// and what those name could then never be removed from here.
pub fn load_installed() -> Result<Vec<Installed>, String> {
    let path = installed_path()?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<InstalledFile>(&text)
            .map(|file| file.items)
            .map_err(|e| format!("{} does not read: {e}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn save_installed(items: &[Installed]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&InstalledFile { items: items.to_vec() }).map_err(|e| e.to_string())?;
    app_dir::write_private(&installed_path()?, text.as_bytes())
}

/// A folder entry's file names as the hash takes them: the ones the record
/// lists, read from under `target`.
fn folder_hash(target: &Path, files: &[String]) -> String {
    let read: Result<Vec<Vec<u8>>, _> = files.iter().map(|file| fs::read(target.join(file))).collect();
    match read {
        Ok(bytes) => content_hash(&files.iter().map(String::as_str).zip(bytes.iter().map(Vec::as_slice)).collect::<Vec<_>>()),
        // A file the install wrote is gone: not what was written.
        Err(_) => String::new(),
    }
}

/// [`content_hash`] of what is at `target` now, `None` when nothing is: a
/// folder hashed over `files`, a single file as itself.
pub fn hash_at(target: &Path, folder: bool, files: &[String]) -> Option<String> {
    // Not `exists()`: a dangling symlink in the place is still something there.
    fs::symlink_metadata(target).ok()?;
    if folder {
        return Some(folder_hash(target, files));
    }
    Some(fs::read(target).map_or_else(|_| String::new(), |bytes| content_hash(&[("", &bytes)])))
}

/// Writes an entry into its place, never half of it: a folder is written in
/// full beside its place, then renamed in; a single file goes through
/// [`app_dir::write_private`]. Refuses when something is there already, and a
/// file name that is not a plain relative path — the index's parser has
/// checked them, and this is where they would escape.
pub fn place(target: &Path, folder: bool, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    if fs::symlink_metadata(target).is_ok() {
        return Err(format!("{} is already there", target.display()));
    }
    if !folder {
        let [(_, bytes)] = files else { return Err("a one-file entry with other than one file".into()) };
        return app_dir::write_private(target, bytes);
    }
    if let Some((bad, _)) = files.iter().find(|(name, _)| !Path::new(name).components().all(|c| matches!(c, Component::Normal(_)))) {
        return Err(format!("\"{bad}\" is not a path inside the folder"));
    }
    let parent = target.parent().ok_or_else(|| format!("{} has no parent", target.display()))?;
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("entry");
    // Hidden, so a scan between here and the rename never lists it.
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let written = files.iter().try_for_each(|(file, bytes)| {
        let path = temp.join(file);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, bytes)
    });
    let renamed = written.and_then(|()| fs::rename(&temp, target));
    renamed.map_err(|e| {
        let _ = fs::remove_dir_all(&temp);
        format!("could not write {}: {e}", target.display())
    })
}

/// Deletes an entry's place — a folder with all in it, or the file. Nothing
/// there is not an error: the user may have deleted it already.
pub fn remove(target: &Path, folder: bool) -> Result<(), String> {
    let removed = if folder { fs::remove_dir_all(target) } else { fs::remove_file(target) };
    match removed {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(format!("could not remove {}: {e}", target.display())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::ItemKind;
    use crate::testing::{temp_dir, with_app_dir};

    fn cached(source: &str) -> CachedIndex {
        CachedIndex { source: source.into(), etag: Some("\"e\"".into()), fetched_at: 7, text: "{}".into() }
    }

    #[test]
    fn keeps_one_index_per_source() {
        with_app_dir("catalog-store", || {
            assert_eq!(load("https://a.example/"), None);
            save(&cached("https://a.example/")).unwrap();
            save(&cached("https://b.example/")).unwrap();
            assert_eq!(load("https://a.example/"), Some(cached("https://a.example/")));
            assert_eq!(load("https://b.example/").unwrap().source, "https://b.example/");
        });
    }

    #[test]
    fn a_file_that_no_longer_reads_is_nothing_kept() {
        with_app_dir("catalog-store-broken", || {
            let at = path("https://a.example/").unwrap();
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(&at, "{").unwrap();
            assert_eq!(load("https://a.example/"), None);
            // Another source's copy under this name — a collision, or a hand edit — is not this one's.
            fs::write(&at, serde_json::to_string(&cached("https://b.example/")).unwrap()).unwrap();
            assert_eq!(load("https://a.example/"), None);
        });
    }

    fn files(list: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        list.iter().map(|(name, text)| (name.to_string(), text.as_bytes().to_vec())).collect()
    }

    #[test]
    fn a_folder_is_placed_whole_and_hashed_as_written() {
        let root = temp_dir("catalog-place-folder");
        let target = root.join("skills").join("review");
        let written = files(&[("SKILL.md", "# Review"), ("refs/checklist.md", "- bugs")]);
        place(&target, true, &written).unwrap();
        assert_eq!(fs::read_to_string(target.join("refs/checklist.md")).unwrap(), "- bugs");
        // Nothing left beside it.
        assert_eq!(fs::read_dir(root.join("skills")).unwrap().count(), 1);

        let names = vec!["SKILL.md".to_string(), "refs/checklist.md".to_string()];
        let expected = content_hash(&[("SKILL.md", b"# Review"), ("refs/checklist.md", b"- bugs")]);
        assert_eq!(hash_at(&target, true, &names), Some(expected.clone()));
        fs::write(target.join("SKILL.md"), "# Mine").unwrap();
        assert_ne!(hash_at(&target, true, &names), Some(expected.clone()));
        fs::remove_file(target.join("SKILL.md")).unwrap();
        assert_eq!(hash_at(&target, true, &names), Some(String::new()));

        assert!(place(&target, true, &written).unwrap_err().contains("already there"));
        remove(&target, true).unwrap();
        assert_eq!(hash_at(&target, true, &names), None);
        remove(&target, true).unwrap();
    }

    #[test]
    fn a_name_that_leaves_the_folder_writes_nothing() {
        let root = temp_dir("catalog-place-escape");
        let target = root.join("skills").join("x");
        for bad in ["../../escaped.md", "/tmp/escaped.md", "a/../../escaped.md"] {
            let err = place(&target, true, &files(&[("SKILL.md", "x"), (bad, "x")])).unwrap_err();
            assert!(err.contains("not a path inside"), "{bad}: {err}");
        }
        assert!(!root.join("escaped.md").exists());
        assert!(fs::symlink_metadata(&target).is_err());
    }

    #[test]
    fn a_folder_that_fails_halfway_leaves_nothing_beside_its_place() {
        let root = temp_dir("catalog-place-half");
        let target = root.join("skills").join("x");
        // `a` is written as a file, then needed as a folder.
        assert!(place(&target, true, &files(&[("a", "x"), ("a/b.md", "x")])).is_err());
        assert_eq!(fs::read_dir(root.join("skills")).unwrap().count(), 0);
    }

    #[test]
    fn a_single_file_is_placed_and_hashed_as_itself() {
        let root = temp_dir("catalog-place-file");
        let target = root.join("commands").join("changelog.md");
        place(&target, false, &files(&[("", "Add a line")])).unwrap();
        assert_eq!(hash_at(&target, false, &[]), Some(content_hash(&[("", b"Add a line")])));
        assert!(place(&target, false, &files(&[("", "again")])).is_err());
        assert!(place(&root.join("other.md"), false, &files(&[("", "a"), ("", "b")])).is_err());
        remove(&target, false).unwrap();
        assert_eq!(hash_at(&target, false, &[]), None);
    }

    #[test]
    fn the_install_record_round_trips_and_a_broken_one_is_an_error() {
        with_app_dir("catalog-installed", || {
            assert_eq!(load_installed(), Ok(Vec::new()));
            let record = Installed {
                source: "https://a.example/".into(),
                kind: ItemKind::Runbook,
                id: "tls".into(),
                role: Some("kubernetes".into()),
                rev: "1".into(),
                files: Vec::new(),
                hash: "h".into(),
                hooks: Default::default(),
            };
            save_installed(&[record.clone()]).unwrap();
            assert_eq!(load_installed(), Ok(vec![record]));
            fs::write(installed_path().unwrap(), "{").unwrap();
            assert!(load_installed().unwrap_err().contains("does not read"));
        });
    }
}
