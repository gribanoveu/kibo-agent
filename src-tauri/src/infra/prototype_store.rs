//! `<app dir>/prototypes/<chat id>/`: a Prototype chat's folder
//! (`docs/31-prototype-role.md`). `files/` is what its tools work in; the
//! presented versions will sit beside it, out of their reach. Made on the
//! chat's first turn, gone with the chat.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::prototype::{self, Asset, Assets, PrototypeError, ASSETS};
use crate::infra::app_dir;

const DIR: &str = "prototypes";
const FILES: &str = "files";
const VERSIONS: &str = "versions";
/// The working folder while a restore swaps it out.
const OLD: &str = ".files.old";

/// The most one version may hold. Pages are kilobytes; a folder past this
/// has something in it the page does not need.
pub const MAX_VERSION_BYTES: u64 = 5 * 1024 * 1024;

/// The chat's working folder, made if missing.
pub fn files_dir(chat_id: &str) -> Result<PathBuf, PrototypeError> {
    files_dir_in(&app_dir::ensure().map_err(PrototypeError::AppDir)?, chat_id)
}

/// The chat's working folder if it has one: a read makes nothing.
pub fn existing_files_dir(chat_id: &str) -> Result<Option<PathBuf>, PrototypeError> {
    let chat = chat_dir(&app_dir::dir()?, chat_id).ok_or_else(|| PrototypeError::NotAChatId(chat_id.to_string()))?;
    Ok(Some(chat.join(FILES)).filter(|files| files.is_dir()))
}

/// Removes the chat's folder with everything in it. A chat that never had
/// one — any but a Prototype chat — has nothing to remove.
pub fn remove(chat_id: &str) -> Result<(), PrototypeError> {
    remove_in(&app_dir::dir()?, chat_id)
}

/// Keeps the chat's working folder `files` as its next version —
/// `versions/<n>` beside it — and returns `n`, one past the highest there is.
/// Copied whole, so an old version shows with its own styles and pictures;
/// links are left out, so a version holds the chat's own files and nothing
/// they point at. Written aside and renamed into place: a version is all
/// there or not there.
pub fn snapshot(files: &Path) -> Result<u32, PrototypeError> {
    let chat = chat_of(files).ok_or_else(|| PrototypeError::NotAChatFolder(files.display().to_string()))?;
    let versions = chat.join(VERSIONS);
    fs::create_dir_all(&versions).map_err(|source| io(&versions, source))?;
    let n = highest(&versions)? + 1;
    let staging = versions.join(format!(".{n}.partial"));
    // Left by a copy that died part-way.
    let _ = fs::remove_dir_all(&staging);
    let kept = copy_tree(files, &staging, &mut 0).and_then(|()| {
        let version = versions.join(n.to_string());
        fs::rename(&staging, &version).map_err(|source| io(&version, source))
    });
    match kept {
        Ok(()) => Ok(n),
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// The working folder `files` as a version: the latest one when it holds just
/// what the folder does — a page presented again unchanged — else a new one.
pub fn kept(files: &Path) -> Result<u32, PrototypeError> {
    match latest(files)? {
        Some(n) if same_as_version(files, n)? => Ok(n),
        _ => snapshot(files),
    }
}

/// The highest version kept of the chat's working folder `files`; `None` before the first.
pub fn latest(files: &Path) -> Result<Option<u32>, PrototypeError> {
    let versions = versions_of(files)?;
    if !versions.is_dir() {
        return Ok(None);
    }
    Ok(Some(highest(&versions)?).filter(|&n| n > 0))
}

/// Whether the working folder holds exactly what version `n` does — the same
/// files with the same bytes, links left out on both sides as a version leaves them.
pub fn same_as_version(files: &Path, n: u32) -> Result<bool, PrototypeError> {
    let version = version_dir(files, n)?;
    Ok(tree(files)? == tree(&version)?)
}

/// Makes the working folder what version `n` holds: its files back, and
/// whatever the folder gained since gone. Copied aside and swapped in, so a
/// copy that fails leaves the folder as it was.
pub fn replace_with_version(files: &Path, n: u32) -> Result<(), PrototypeError> {
    let version = version_dir(files, n)?;
    let chat = chat_of(files).ok_or_else(|| PrototypeError::NotAChatFolder(files.display().to_string()))?;
    let staging = chat.join(".files.partial");
    let old = chat.join(OLD);
    // Left by a restore that died part-way: the copy goes, and the folder it
    // swapped out is put back before what is left of it is taken for a leftover.
    let _ = fs::remove_dir_all(&staging);
    recover(chat)?;
    let _ = fs::remove_dir_all(&old);
    if let Err(e) = copy_tree(&version, &staging, &mut 0) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    fs::rename(files, &old).map_err(|source| io(files, source))?;
    if let Err(source) = fs::rename(&staging, files) {
        // Put back what was there rather than leave no folder.
        let _ = fs::rename(&old, files);
        return Err(io(files, source));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

fn versions_of(files: &Path) -> Result<PathBuf, PrototypeError> {
    let chat = chat_of(files).ok_or_else(|| PrototypeError::NotAChatFolder(files.display().to_string()))?;
    Ok(chat.join(VERSIONS))
}

/// Version `n` of the working folder `files`, or why there is none.
pub(crate) fn version_dir(files: &Path, n: u32) -> Result<PathBuf, PrototypeError> {
    let version = versions_of(files)?.join(n.to_string());
    if version.is_dir() { Ok(version) } else { Err(PrototypeError::NoVersion(n)) }
}

/// A folder's files by their path inside it, with their bytes.
fn tree(dir: &Path) -> Result<std::collections::BTreeMap<PathBuf, Vec<u8>>, PrototypeError> {
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) -> Result<(), PrototypeError> {
        for entry in fs::read_dir(dir).map_err(|source| io(dir, source))? {
            let entry = entry.map_err(|source| io(dir, source))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|source| io(&path, source))?;
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                let bytes = fs::read(&path).map_err(|source| io(&path, source))?;
                out.insert(path.strip_prefix(root).unwrap_or(&path).to_path_buf(), bytes);
            }
        }
        Ok(())
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// The chat folder `files` is the working folder of, if it is one.
fn chat_of(files: &Path) -> Option<&Path> {
    let chat = files.parent()?;
    (files.file_name()? == FILES && chat.parent()?.file_name()? == DIR).then_some(chat)
}

fn highest(versions: &Path) -> Result<u32, PrototypeError> {
    let entries = fs::read_dir(versions).map_err(|source| io(versions, source))?;
    Ok(entries.flatten().filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok()).max().unwrap_or(0))
}

fn copy_tree(from: &Path, to: &Path, bytes: &mut u64) -> Result<(), PrototypeError> {
    fs::create_dir_all(to).map_err(|source| io(to, source))?;
    for entry in fs::read_dir(from).map_err(|source| io(from, source))? {
        let entry = entry.map_err(|source| io(from, source))?;
        let path = entry.path();
        // Not followed: a link is neither.
        let kind = entry.file_type().map_err(|source| io(&path, source))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&path, &target, bytes)?;
        } else if kind.is_file() {
            *bytes += entry.metadata().map_err(|source| io(&path, source))?.len();
            if *bytes > MAX_VERSION_BYTES {
                return Err(PrototypeError::TooBig { limit: MAX_VERSION_BYTES });
            }
            fs::copy(&path, &target).map_err(|source| io(&target, source))?;
        }
    }
    Ok(())
}

fn files_dir_in(base: &Path, chat_id: &str) -> Result<PathBuf, PrototypeError> {
    let chat = chat_dir(base, chat_id).ok_or_else(|| PrototypeError::NotAChatId(chat_id.to_string()))?;
    recover(&chat)?;
    let dir = chat.join(FILES);
    fs::create_dir_all(&dir).map_err(|source| io(&dir, source))?;
    Ok(dir)
}

/// A restore that died between its two renames leaves the working folder as
/// `.files.old` and no `files/`. It is put back before anything makes an empty
/// `files/` in its place — after which the next restore would delete it as a
/// leftover, and the chat's unpresented work with it.
fn recover(chat: &Path) -> Result<(), PrototypeError> {
    let (files, old) = (chat.join(FILES), chat.join(OLD));
    if !files.exists() && old.is_dir() {
        fs::rename(&old, &files).map_err(|source| io(&files, source))?;
    }
    Ok(())
}

fn remove_in(base: &Path, chat_id: &str) -> Result<(), PrototypeError> {
    // An id no folder could have been made for has none.
    let Some(dir) = chat_dir(base, chat_id) else { return Ok(()) };
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io(&dir, source)),
    }
}

fn io(path: &Path, source: std::io::Error) -> PrototypeError {
    PrototypeError::Io { path: path.display().to_string(), source }
}

/// The pictures in the working folder's `assets/`, by name, and how full
/// the folder is. Links are left out, as a version leaves them.
pub fn assets(files: &Path) -> Result<Assets, PrototypeError> {
    let dir = files.join(ASSETS);
    let mut assets = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(&dir).map_err(|source| io(&dir, source))? {
            let entry = entry.map_err(|source| io(&dir, source))?;
            let kind = entry.file_type().map_err(|source| io(&entry.path(), source))?;
            if kind.is_file() {
                let bytes = entry.metadata().map_err(|source| io(&entry.path(), source))?.len();
                assets.push(asset(&entry.path(), entry.file_name().to_string_lossy().into_owned(), bytes));
            }
        }
    }
    assets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Assets { assets, folder_bytes: size(files)?, limit_bytes: MAX_VERSION_BYTES })
}

/// Writes a picture into `assets/` under a name of its own made from `hint`
/// ([`prototype::asset_name`]) — never over a file already there.
pub fn add_asset(files: &Path, hint: &str, extension: &str, bytes: &[u8]) -> Result<Asset, PrototypeError> {
    let dir = files.join(ASSETS);
    fs::create_dir_all(&dir).map_err(|source| io(&dir, source))?;
    let name = prototype::asset_name(hint, extension, |name| dir.join(name).exists());
    let path = dir.join(&name);
    fs::write(&path, bytes).map_err(|source| io(&path, source))?;
    Ok(asset(&path, name, bytes.len() as u64))
}

/// Deletes the picture `name` from `assets/`. The name comes from the window:
/// one plain file name, of a file that is there, or nothing is touched.
pub fn remove_asset(files: &Path, name: &str) -> Result<(), PrototypeError> {
    let plain = matches!(Path::new(name).components().collect::<Vec<_>>()[..], [std::path::Component::Normal(one)] if one == name);
    let path = files.join(ASSETS).join(name);
    if !(plain && path.is_file()) {
        return Err(PrototypeError::NotAnAsset(name.to_string()));
    }
    fs::remove_file(&path).map_err(|source| io(&path, source))
}

fn asset(path: &Path, name: String, bytes: u64) -> Asset {
    let (width, height) = image::image_dimensions(path).map_or((None, None), |(w, h)| (Some(w), Some(h)));
    Asset { name, bytes, width, height }
}

/// The bytes of the files under `dir` — what a version of it would copy.
fn size(dir: &Path) -> Result<u64, PrototypeError> {
    let mut total = 0;
    for entry in fs::read_dir(dir).map_err(|source| io(dir, source))? {
        let entry = entry.map_err(|source| io(dir, source))?;
        let kind = entry.file_type().map_err(|source| io(&entry.path(), source))?;
        if kind.is_dir() {
            total += size(&entry.path())?;
        } else if kind.is_file() {
            total += entry.metadata().map_err(|source| io(&entry.path(), source))?.len();
        }
    }
    Ok(total)
}

/// The id comes from the window and becomes a path: one segment of plain
/// characters — what `crypto.randomUUID` makes — or no folder at all. `base`
/// is the app directory; the protocol asks here too, so the rule is one.
pub(crate) fn chat_dir(base: &Path, chat_id: &str) -> Option<PathBuf> {
    let plain = !chat_id.is_empty()
        && chat_id.len() <= 64
        && chat_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| base.join(DIR).join(chat_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    const ID: &str = "0b9d6a0e-4f4e-4c8e-9a51-3c2f1d7e8a10";

    #[test]
    fn a_chat_gets_a_folder_of_its_own_under_the_app_dir() {
        let base = temp_dir("prototype-store");
        let dir = files_dir_in(base.as_path(), ID).unwrap();
        assert_eq!(dir, base.as_path().join("prototypes").join(ID).join("files"));
        assert!(dir.is_dir());
        // Asked again, the same folder, its files kept.
        fs::write(dir.join("index.html"), "<p>hi</p>").unwrap();
        assert_eq!(files_dir_in(base.as_path(), ID).unwrap(), dir);
        assert!(dir.join("index.html").is_file());
    }

    /// An id is a path segment: anything that could climb out, nest or name
    /// nothing gets no folder, and nothing is made for it.
    #[test]
    fn an_id_that_is_not_one_plain_segment_gets_no_folder() {
        let base = temp_dir("prototype-store");
        let long = "a".repeat(65);
        for id in ["", ".", "..", "../x", "a/b", "a\\b", "a b", "a.b", "é", long.as_str()] {
            assert!(files_dir_in(base.as_path(), id).is_err(), "{id:?}");
        }
        assert!(!base.join("prototypes").exists(), "nothing made for a refused id");
    }

    #[test]
    fn removing_a_chat_takes_its_whole_folder() {
        let base = temp_dir("prototype-store");
        let dir = files_dir_in(base.as_path(), ID).unwrap();
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("assets").join("a.svg"), "<svg/>").unwrap();
        let other = files_dir_in(base.as_path(), "other-chat").unwrap();

        remove_in(base.as_path(), ID).unwrap();
        assert!(!base.as_path().join("prototypes").join(ID).exists());
        assert!(other.is_dir(), "another chat's folder stays");
    }

    fn versions(base: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(base.join("prototypes").join(ID).join("versions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Each version is the whole folder as it was, numbered on from the last;
    /// the working folder goes on changing without touching it.
    #[test]
    fn a_version_keeps_the_whole_folder_as_it_was() {
        let base = temp_dir("prototype-snapshot");
        let files = files_dir_in(&base, ID).unwrap();
        fs::create_dir_all(files.join("assets")).unwrap();
        fs::write(files.join("index.html"), "one").unwrap();
        fs::write(files.join("assets").join("style.css"), "a{}").unwrap();

        assert_eq!(snapshot(&files).unwrap(), 1);
        fs::write(files.join("index.html"), "two").unwrap();
        assert_eq!(snapshot(&files).unwrap(), 2);

        let kept = base.join("prototypes").join(ID).join("versions");
        assert_eq!(fs::read_to_string(kept.join("1").join("index.html")).unwrap(), "one");
        assert_eq!(fs::read_to_string(kept.join("1").join("assets").join("style.css")).unwrap(), "a{}");
        assert_eq!(fs::read_to_string(kept.join("2").join("index.html")).unwrap(), "two");
        assert_eq!(versions(&base), ["1", "2"]);
    }

    /// One past the highest, whatever is missing below it — and a copy that
    /// died part-way is neither counted nor left.
    #[test]
    fn the_next_version_is_one_past_the_highest() {
        let base = temp_dir("prototype-numbering");
        let files = files_dir_in(&base, ID).unwrap();
        fs::write(files.join("index.html"), "x").unwrap();
        let kept = base.join("prototypes").join(ID).join("versions");
        for n in ["1", "7", "notes", ".8.partial"] {
            fs::create_dir_all(kept.join(n)).unwrap();
        }
        fs::write(kept.join(".8.partial").join("stale.html"), "from a copy that died").unwrap();
        assert_eq!(snapshot(&files).unwrap(), 8);
        assert_eq!(versions(&base), ["1", "7", "8", "notes"]);
        assert!(!kept.join("8").join("stale.html").exists(), "nothing of the dead copy in the version");
    }

    /// Too big is refused with why, and leaves nothing behind.
    #[test]
    fn a_folder_over_the_limit_is_not_kept() {
        let base = temp_dir("prototype-too-big");
        let files = files_dir_in(&base, ID).unwrap();
        fs::write(files.join("index.html"), "x").unwrap();
        fs::write(files.join("video.bin"), vec![0u8; MAX_VERSION_BYTES as usize]).unwrap();
        let refused = snapshot(&files).unwrap_err().to_string();
        assert!(refused.contains("over 5 MB"), "{refused}");
        assert!(versions(&base).is_empty());
    }

    /// A link in the folder is not followed into the version.
    #[cfg(unix)]
    #[test]
    fn a_link_in_the_folder_is_left_out_of_the_version() {
        let base = temp_dir("prototype-link");
        let files = files_dir_in(&base, ID).unwrap();
        fs::write(files.join("index.html"), "x").unwrap();
        fs::write(base.join("secret.txt"), "key").unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), files.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&base, files.join("up")).unwrap();
        snapshot(&files).unwrap();
        let one = base.join("prototypes").join(ID).join("versions").join("1");
        assert!(one.join("index.html").is_file());
        assert!(!one.join("link.txt").exists() && !one.join("up").exists());
    }

    /// A restore that died between its two renames left the working folder as
    /// `.files.old`: it comes back as the folder — on the next turn's way in,
    /// or at the next restore — and is never deleted as a leftover.
    #[test]
    fn a_folder_swapped_out_by_a_dead_restore_comes_back() {
        let base = temp_dir("prototype-recover");
        let files = files_dir_in(&base, ID).unwrap();
        fs::write(files.join("index.html"), "one").unwrap();
        snapshot(&files).unwrap();
        fs::write(files.join("index.html"), "unpresented").unwrap();
        let old = files.parent().unwrap().join(OLD);
        // The swap's first rename, and nothing after it.
        fs::rename(&files, &old).unwrap();

        assert_eq!(files_dir_in(&base, ID).unwrap(), files);
        assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "unpresented");
        assert!(!old.exists());

        fs::rename(&files, &old).unwrap();
        replace_with_version(&files, 1).unwrap();
        assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "one");
        assert!(!old.exists());
    }

    /// Only a Prototype chat's working folder has a `versions/` to keep one in.
    #[test]
    fn a_folder_that_is_not_a_chats_is_refused() {
        let base = temp_dir("prototype-not-chat");
        for folder in [base.clone(), base.join("files"), base.join("prototypes").join(ID)] {
            fs::create_dir_all(&folder).unwrap();
            assert!(matches!(snapshot(&folder), Err(PrototypeError::NotAChatFolder(_))), "{folder:?}");
        }
    }

    /// Every chat is deleted through here, and most never had a folder.
    #[test]
    fn removing_a_chat_without_a_folder_or_with_an_odd_id_is_nothing() {
        let base = temp_dir("prototype-store");
        let kept = files_dir_in(base.as_path(), ID).unwrap();
        remove_in(base.as_path(), "never-had-one").unwrap();
        remove_in(base.as_path(), "..").unwrap();
        remove_in(base.as_path(), "").unwrap();
        assert!(kept.is_dir());
        assert!(base.as_path().exists());
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// Added under names of their own, listed by name with their sizes, the
    /// folder's total counting the page too.
    #[test]
    fn pictures_go_into_assets_under_names_of_their_own() {
        let files = files_dir_in(temp_dir("prototype-assets").as_path(), ID).unwrap();
        assert_eq!(assets(&files).unwrap(), Assets { assets: vec![], folder_bytes: 0, limit_bytes: MAX_VERSION_BYTES });

        fs::write(files.join("index.html"), "<p>hi</p>").unwrap();
        let logo = add_asset(&files, "/Users/me/Our Logo.png", "png", &png(4, 3)).unwrap();
        let again = add_asset(&files, "Our Logo.png", "png", &png(2, 2)).unwrap();
        assert_eq!((logo.name.as_str(), again.name.as_str()), ("our-logo.png", "our-logo-2.png"), "never over one already there");
        assert_eq!((logo.width, logo.height), (Some(4), Some(3)));
        fs::write(files.join("assets").join("a-note.txt"), "x").unwrap();
        fs::create_dir_all(files.join("assets").join("a-folder")).unwrap();

        let listed = assets(&files).unwrap();
        let names: Vec<&str> = listed.assets.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["a-note.txt", "our-logo-2.png", "our-logo.png"]);
        assert_eq!((listed.assets[0].width, &listed.assets[2]), (None, &logo), "a file not a picture has no size in pixels");
        let total: u64 = listed.assets.iter().map(|a| a.bytes).sum::<u64>() + "<p>hi</p>".len() as u64;
        assert_eq!(listed.folder_bytes, total);
    }

    /// Only a plain name of a file in `assets/` is deleted: nothing a name
    /// could climb out with, no folder, nothing that is not there.
    #[test]
    fn a_picture_is_removed_by_its_plain_name_only() {
        let files = files_dir_in(temp_dir("prototype-assets-remove").as_path(), ID).unwrap();
        let logo = add_asset(&files, "logo.png", "png", &png(1, 1)).unwrap();
        fs::write(files.join("index.html"), "<p>hi</p>").unwrap();
        fs::create_dir_all(files.join("assets").join("sub")).unwrap();

        for bad in ["../index.html", "sub", "sub/../logo.png", "", "missing.png", "/etc/hosts"] {
            assert!(matches!(remove_asset(&files, bad), Err(PrototypeError::NotAnAsset(_))), "{bad}");
        }
        assert!(files.join("index.html").is_file());
        remove_asset(&files, &logo.name).unwrap();
        assert!(!files.join("assets").join(&logo.name).exists());
    }

}
