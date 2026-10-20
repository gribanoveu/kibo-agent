//! The pictures a user puts in a Prototype chat's folder (`docs/31-prototype-role.md`,
//! P-10): in `assets/`, where a page names them and the model lists them.
//! Added and removed while nothing else works in the folder — a restore swaps
//! it whole, and a turn's model is reading it.

use base64::Engine;

use crate::domain::image::{ImageMediaType, ImagePart};
use crate::domain::prototype::{Asset, Assets, PrototypeError};
use crate::infra::prototype_store;
use crate::services::prototype_versions::claim;

/// What the chat's `assets/` holds — nothing, for a chat with no folder yet,
/// which a read does not make. Read without holding the folder: the drawer
/// asks while a turn runs, and a list a moment old costs nothing.
pub fn list(chat_id: &str) -> Result<Assets, PrototypeError> {
    match prototype_store::existing_files_dir(chat_id)? {
        Some(files) => prototype_store::assets(&files),
        None => Ok(Assets { assets: Vec::new(), folder_bytes: 0, limit_bytes: prototype_store::MAX_VERSION_BYTES }),
    }
}

/// Puts `image` — sanitized when it was attached — into `assets/`, named
/// after `name`. The bytes come back from the window, so they are checked to
/// be the picture they say they are before anything is written.
pub fn add(chat_id: &str, name: &str, image: &ImagePart) -> Result<Asset, PrototypeError> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(&image.data).map_err(|_| PrototypeError::NotAPicture)?;
    let (format, extension) = match image.media_type {
        ImageMediaType::Png => (image::ImageFormat::Png, "png"),
        ImageMediaType::Jpeg => (image::ImageFormat::Jpeg, "jpg"),
    };
    if image::guess_format(&bytes).ok() != Some(format) {
        return Err(PrototypeError::NotAPicture);
    }
    let _claim = claim(chat_id)?;
    prototype_store::add_asset(&prototype_store::files_dir(chat_id)?, name, extension, &bytes)
}

/// Deletes the picture `name` from `assets/`. Versions keep their copies.
pub fn remove(chat_id: &str, name: &str) -> Result<(), PrototypeError> {
    let _claim = claim(chat_id)?;
    prototype_store::remove_asset(&prototype_store::files_dir(chat_id)?, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "5d0c1b2a-3e4f-4a5b-8c6d-7e8f9a0b1c2d";

    fn part(media_type: ImageMediaType, bytes: &[u8]) -> ImagePart {
        ImagePart { media_type, data: base64::engine::general_purpose::STANDARD.encode(bytes), width: 1, height: 1 }
    }

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3, 2).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    /// Bytes that are not the picture they claim to be write nothing.
    #[test]
    fn only_the_picture_it_says_it_is_is_written() {
        crate::testing::with_app_dir("prototype-assets-add", || {
            let refused = [
                part(ImageMediaType::Jpeg, &png()),
                part(ImageMediaType::Png, b"<script>alert(1)</script>"),
                ImagePart { data: "not base64!".into(), ..part(ImageMediaType::Png, &png()) },
            ];
            for image in &refused {
                assert!(matches!(add(ID, "x.png", image), Err(PrototypeError::NotAPicture)), "{:?}", image.media_type);
            }
            assert!(list(ID).unwrap().assets.is_empty());

            let added = add(ID, "Logo.png", &part(ImageMediaType::Png, &png())).unwrap();
            assert_eq!((added.name.as_str(), added.width, added.height), ("logo.png", Some(3), Some(2)));
            assert_eq!(list(ID).unwrap().assets, [added]);
        });
    }

    /// Asking what a chat with no folder holds makes no folder.
    #[test]
    fn a_list_makes_no_folder() {
        crate::testing::with_app_dir("prototype-assets-list", || {
            assert!(list(ID).unwrap().assets.is_empty());
            let chat = crate::infra::app_dir::dir().unwrap().join("prototypes").join(ID);
            assert!(!chat.exists(), "{}", chat.display());
            assert!(matches!(list("../x"), Err(PrototypeError::NotAChatId(_))));
        });
    }

    /// While a turn or a restore holds the folder, nothing is added or removed.
    #[test]
    fn a_folder_in_use_takes_no_pictures() {
        crate::testing::with_app_dir("prototype-assets-busy", || {
            let added = add(ID, "logo.png", &part(ImageMediaType::Png, &png())).unwrap();
            let held = claim(ID).unwrap();
            assert!(matches!(add(ID, "b.png", &part(ImageMediaType::Png, &png())), Err(PrototypeError::Busy)));
            assert!(matches!(remove(ID, &added.name), Err(PrototypeError::Busy)));
            assert_eq!(list(ID).unwrap().assets.len(), 1, "listed while held");
            drop(held);
            remove(ID, &added.name).unwrap();
            assert!(list(ID).unwrap().assets.is_empty());
        });
    }
}
