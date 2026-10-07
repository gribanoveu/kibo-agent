//! A picture the user attached for the model, after sanitizing — and the ways
//! sanitizing refuses one.
//!
//! An `ImagePart` is only ever made by `services::image_sanitize`: pixels
//! decoded from the user's file and encoded again by us, with nothing of the
//! original file carried over. `media_type` admits PNG and JPEG and nothing
//! else, so a saved chat edited by hand cannot slip another format past it.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The only two formats sanitizing writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
}

impl ImageMediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePart {
    pub media_type: ImageMediaType,
    /// The encoded file, base64 — what both providers' wires carry as is.
    pub data: String,
    /// After orientation and downscaling: what the model is sent, and what a
    /// token estimate is made from.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImageError {
    #[error("the image is {bytes} bytes, over the {limit} allowed")]
    TooLarge { bytes: usize, limit: usize },
    /// `detected` names the format when its signature is a known one.
    #[error("{} is not supported: save the image as PNG or JPEG", detected.as_deref().unwrap_or("this file"))]
    UnsupportedFormat { detected: Option<String> },
    #[error("the image is {width}×{height}, over {limit} pixels on a side")]
    DimensionsTooLarge { width: u32, height: u32, limit: u32 },
    #[error("the image could not be read: {0}")]
    Decode(String),
    #[error("the image could not be encoded: {0}")]
    Encode(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire and the saved chat spell the type the way both providers do.
    #[test]
    fn the_media_type_is_spelled_as_a_mime_type() {
        let json = serde_json::to_string(&ImageMediaType::Jpeg).expect("serializes");
        assert_eq!(json, r#""image/jpeg""#);
        assert_eq!(ImageMediaType::Png.as_str(), "image/png");
        assert_eq!(ImageMediaType::Jpeg.as_str(), "image/jpeg");
    }

    /// The type is the guard against a format sanitizing never wrote: a part
    /// claiming WebP or SVG does not load.
    #[test]
    fn another_media_type_does_not_load() {
        for other in ["image/webp", "image/svg+xml", "image/gif", "text/html"] {
            let json = format!(r#"{{"mediaType":"{other}","data":"","width":1,"height":1}}"#);
            assert!(serde_json::from_str::<ImagePart>(&json).is_err(), "{other} loaded");
        }
    }

    #[test]
    fn an_unsupported_format_says_which_and_what_to_do() {
        let named = ImageError::UnsupportedFormat { detected: Some("WebP".into()) };
        assert_eq!(named.to_string(), "WebP is not supported: save the image as PNG or JPEG");
        let unnamed = ImageError::UnsupportedFormat { detected: None };
        assert_eq!(unnamed.to_string(), "this file is not supported: save the image as PNG or JPEG");
    }
}
