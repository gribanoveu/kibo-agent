//! A Prototype chat's folder (`docs/31-prototype-role.md`): what a restore
//! did, the pictures the user put in it, and what can go wrong with it.

use serde::Serialize;
use thiserror::Error;

use super::chat_record::ChatError;
use super::settings::AppDirError;

#[derive(Debug, Error)]
pub enum PrototypeError {
    /// The id came from the window and is not one plain path segment, so no
    /// folder is made for it.
    #[error("not a chat id: {0:?}")]
    NotAChatId(String),
    /// The app directory itself could not be made.
    #[error("{0}")]
    AppDir(String),
    #[error(transparent)]
    NoAppDir(#[from] AppDirError),
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error(transparent)]
    Chat(#[from] ChatError),
    /// `present` outside a Prototype chat's working folder: there is no
    /// `versions/` beside it to keep one in.
    #[error("present works in a Prototype chat's folder only, and {0} is not one")]
    NotAChatFolder(String),
    /// Worded for the model: what to do about it.
    #[error("the folder is over {} MB, the most a version keeps — delete what the page does not use, or keep large files out of it", limit / (1024 * 1024))]
    TooBig { limit: u64 },
    #[error("there is no version {0} of this chat's folder")]
    NoVersion(u32),
    /// "Save as" named a page that is not in the folder or the version.
    #[error("there is no page {0} to save")]
    NoPage(String),
    /// A turn or a restore already works in the chat's folder: one at a time,
    /// or a restore swaps the folder out from under a write.
    #[error("the chat's folder is in use — wait for the turn or the restore under way to finish")]
    Busy,
    /// A name from the window that is not a file in `assets/`.
    #[error("there is no picture {0:?} in {ASSETS}/")]
    NotAnAsset(String),
    /// What the window sent as a picture is not the PNG or JPEG it says it is.
    #[error("that is not a PNG or JPEG picture")]
    NotAPicture,
}

/// What restoring a version did (P-6): the version put back, the new one the
/// folder is kept as now, and — when the folder had changed since its last
/// version — the version it was kept as first, so nothing is lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub from: u32,
    pub version: u32,
    pub before: Option<u32>,
}

/// The folder the user's pictures go in, inside the chat's working folder:
/// a page names them `assets/<name>`.
pub const ASSETS: &str = "assets";

/// A file in `assets/`, as the drawer lists it. The size in pixels is there
/// for a PNG or JPEG; a file the model wrote there may be something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// What `assets/` holds, and how full the working folder is against what a
/// version may keep — the pictures are most of what fills it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assets {
    pub assets: Vec<Asset>,
    pub folder_bytes: u64,
    pub limit_bytes: u64,
}

/// A name for a picture in `assets/`: the stem of what it was called, in
/// lowercase ASCII letters, digits and dashes — a page and the model write it
/// in a URL — with `extension`, and a number after the stem while `taken`
/// says the name is in use. "image" when nothing of the stem is left.
pub fn asset_name(hint: &str, extension: &str, taken: impl Fn(&str) -> bool) -> String {
    let file = hint.rsplit(['/', '\\']).next().unwrap_or(hint);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let mut cleaned = String::new();
    for c in stem.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            cleaned.push(c);
        } else if !cleaned.is_empty() && !cleaned.ends_with('-') {
            cleaned.push('-');
        }
    }
    let cleaned: String = cleaned.chars().take(40).collect();
    let stem = match cleaned.trim_end_matches('-') {
        "" => "image",
        stem => stem,
    };
    let mut name = format!("{stem}.{extension}");
    let mut n = 2;
    while taken(&name) {
        name = format!("{stem}-{n}.{extension}");
        n += 1;
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_named_after_its_file_in_letters_a_url_takes() {
        let free = |_: &str| false;
        assert_eq!(asset_name("/Users/me/Desktop/Our Logo (final).PNG", "png", free), "our-logo-final.png");
        assert_eq!(asset_name("C:\\pics\\team_photo.jpeg", "jpg", free), "team-photo.jpg");
        assert_eq!(asset_name("логотип.png", "png", free), "image.png", "nothing a URL takes plainly is left");
        assert_eq!(asset_name("", "png", free), "image.png");
        assert_eq!(asset_name("hero.v2.png", "png", free), "hero-v2.png", "the extension is what follows the last dot");
        assert_eq!(asset_name("--a--b--.png", "png", free), "a-b.png");
        assert_eq!(asset_name(&"x".repeat(80), "png", free), format!("{}.png", "x".repeat(40)));
    }

    #[test]
    fn a_name_in_use_gets_the_next_number() {
        let taken = |name: &str| ["logo.png", "logo-2.png"].contains(&name);
        assert_eq!(asset_name("logo.png", "png", taken), "logo-3.png");
        assert_eq!(asset_name("logo.png", "jpg", taken), "logo.jpg", "another extension is another name");
    }
}
