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

/// What a picture is estimated to cost, at least and at most. Measured on
/// DeepSeek (`docs/30-vision.md`, V-0): ~200 tokens for 64×32, ~940 for
/// 1568×1000. Anthropic charges about `w·h/750` and scales to 1.15 MP, about
/// 1600. The formula is Anthropic's and the floor DeepSeek's: an estimate
/// that errs high only compacts a little early.
const MIN_TOKENS: usize = 200;
const MAX_TOKENS: usize = 1600;
const PIXELS_PER_TOKEN: usize = 750;

impl ImagePart {
    pub fn estimate_tokens(&self) -> usize {
        (self.width as usize * self.height as usize).div_ceil(PIXELS_PER_TOKEN).clamp(MIN_TOKENS, MAX_TOKENS)
    }

    /// The picture as a line of text, where only text goes: a summary
    /// request, an exported chat.
    pub fn note(&self) -> String {
        format!("[image {}×{}]", self.width, self.height)
    }

    /// Sent in its place to a provider not set to accept pictures — saying
    /// so, so the model can tell the user why it did not see it.
    pub fn omitted_note(&self) -> String {
        format!("[image {}×{} omitted: this provider is not set to accept images]", self.width, self.height)
    }
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
    /// A dropped file that could not be read — gone, a folder, no permission.
    #[error("the file could not be read: {0}")]
    Read(String),
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

    fn part(width: u32, height: u32) -> ImagePart {
        ImagePart { media_type: ImageMediaType::Png, data: String::new(), width, height }
    }

    #[test]
    fn a_picture_costs_its_pixels_between_a_floor_and_a_ceiling() {
        assert_eq!(part(64, 32).estimate_tokens(), 200, "DeepSeek's floor");
        assert_eq!(part(600, 400).estimate_tokens(), 320, "w·h/750");
        assert_eq!(part(601, 400).estimate_tokens(), 321, "rounded up");
        assert_eq!(part(1568, 1176).estimate_tokens(), 1600, "Anthropic's ceiling");
    }

    #[test]
    fn a_picture_reads_as_a_line_of_text() {
        assert_eq!(part(1568, 1176).note(), "[image 1568×1176]");
        assert_eq!(
            part(20, 40).omitted_note(),
            "[image 20×40 omitted: this provider is not set to accept images]"
        );
    }

    #[test]
    fn an_unsupported_format_says_which_and_what_to_do() {
        let named = ImageError::UnsupportedFormat { detected: Some("WebP".into()) };
        assert_eq!(named.to_string(), "WebP is not supported: save the image as PNG or JPEG");
        let unnamed = ImageError::UnsupportedFormat { detected: None };
        assert_eq!(unnamed.to_string(), "this file is not supported: save the image as PNG or JPEG");
    }
}
