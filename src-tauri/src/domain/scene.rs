//! A scene the model painted for behind Frost's glass (`docs/55-frost-scenes.md`,
//! decision 8): its title and body checked before it is kept.
//!
//! What keeps a scene from running script or reaching the network is the
//! frame and the policy it is shown under, not this check. The check is there
//! so the model hears that what it wrote would not work — a `<script>`, a
//! picture from a URL — and fixes it, instead of keeping a scene with dead parts.
//! Its words go back to the model as they are.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A scene's body at most: a few hundred moving elements and their styles
/// fit many times over; past it, something is inlined that should not be.
pub const MAX_BYTES: usize = 256 * 1024;
/// A title at most, in characters: a card's name.
pub const MAX_TITLE: usize = 80;
/// Scenes kept at most; past it the user deletes some in Settings.
pub const MAX_SCENES: usize = 50;

/// Tags a scene may not use: `<script>` runs nothing in the frame, `<meta>`
/// (`refresh`) would take the frame to an address past the policy, and the
/// rest reach out, embed or submit.
const FORBIDDEN_TAGS: &[&str] = &["script", "meta", "base", "link", "iframe", "object", "embed", "form"];

/// The addresses a scene may name: the namespaces of inline SVG, which are
/// names and never fetched.
const NAMESPACES: &[&str] = &["http://www.w3.org/2000/svg", "http://www.w3.org/1999/xlink", "http://www.w3.org/1999/xhtml"];

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SceneError {
    #[error("the scene is {0} bytes; keep it under {MAX_BYTES} — draw with CSS and inline SVG, not embedded pictures")]
    TooLarge(usize),
    #[error("a scene needs a title of 1 to {MAX_TITLE} characters")]
    Title,
    #[error("a scene may not use <{0}>: it is shown in a frame that runs no script and reaches no network — send only the body's HTML and CSS, no <html> or <head>")]
    Tag(&'static str),
    #[error("a scene may not have event handlers ({0}=): it runs no script")]
    Handler(String),
    #[error("a scene may not name http: or https: addresses: nothing is fetched, so put pictures inline as data: URLs or draw them")]
    Network,
    #[error("{MAX_SCENES} scenes are kept already; ask the user to delete some in Settings → Appearance")]
    Full,
    #[error("no scene {0}")]
    NotFound(String),
    #[error("not a scene id: {0}")]
    BadId(String),
    #[error("the scenes' store failed: {0}")]
    Store(String),
}

/// A kept scene, as the list shows it; its body is read on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneInfo {
    pub id: String,
    pub title: String,
    /// Milliseconds since the epoch.
    pub created: u64,
}

/// `title` trimmed, if a scene may be kept under it with `body`.
pub fn check<'a>(title: &'a str, body: &str) -> Result<&'a str, SceneError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return Err(SceneError::Title);
    }
    if body.len() > MAX_BYTES {
        return Err(SceneError::TooLarge(body.len()));
    }
    let lower = body.to_ascii_lowercase();
    if let Some(tag) = forbidden_tag(&lower) {
        return Err(SceneError::Tag(tag));
    }
    if let Some(handler) = handler(&lower) {
        return Err(SceneError::Handler(handler));
    }
    let unnamed = NAMESPACES.iter().fold(lower.clone(), |text, ns| text.replace(ns, ""));
    if unnamed.contains("http:") || unnamed.contains("https:") {
        return Err(SceneError::Network);
    }
    Ok(title)
}

/// Room for one more scene beside `kept`.
pub fn room(kept: usize) -> Result<(), SceneError> {
    if kept >= MAX_SCENES { Err(SceneError::Full) } else { Ok(()) }
}

/// `id` if it is a scene's: a UUID in its plain lowercase spelling, so it
/// names one file in the scenes' folder and never a path.
pub fn id(id: &str) -> Result<&str, SceneError> {
    match uuid::Uuid::parse_str(id) {
        Ok(parsed) if parsed.hyphenated().to_string() == id => Ok(id),
        _ => Err(SceneError::BadId(id.to_string())),
    }
}

/// The first forbidden tag opened in `lower`, spaces after `<` or not.
fn forbidden_tag(lower: &str) -> Option<&'static str> {
    lower.match_indices('<').find_map(|(at, _)| {
        let name: String = lower[at + 1..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        FORBIDDEN_TAGS.iter().copied().find(|tag| *tag == name)
    })
}

/// The first `on…=` attribute in `lower`: `on` and letters, after a space,
/// a slash or a quote, before an `=`.
fn handler(lower: &str) -> Option<String> {
    let bytes = lower.as_bytes();
    lower.match_indices("on").find_map(|(at, _)| {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        if !matches!(before, Some(b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'"' | b'\'')) {
            return None;
        }
        let name_end = at + 2 + lower[at + 2..].bytes().take_while(u8::is_ascii_lowercase).count();
        lower[name_end..].trim_start().starts_with('=').then(|| lower[at..name_end].to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"<style>.f{animation:fall 30s linear infinite}</style><div class="f" style="opacity:.5"></div><svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0"/></svg>"#;

    #[test]
    fn a_scene_of_html_and_css_is_kept_under_its_title_trimmed() {
        assert_eq!(check("  Snowy forest ", BODY), Ok("Snowy forest"));
    }

    #[test]
    fn a_title_is_one_to_eighty_characters() {
        assert_eq!(check("   ", BODY), Err(SceneError::Title));
        assert_eq!(check(&"ё".repeat(MAX_TITLE), BODY), Ok("ё".repeat(MAX_TITLE).as_str()));
        assert_eq!(check(&"ё".repeat(MAX_TITLE + 1), BODY), Err(SceneError::Title));
    }

    #[test]
    fn a_body_is_at_most_256_kb() {
        let at = format!("{BODY}{}", " ".repeat(MAX_BYTES - BODY.len()));
        assert_eq!(check("t", &at), Ok("t"));
        assert_eq!(check("t", &format!("{at} ")), Err(SceneError::TooLarge(MAX_BYTES + 1)));
    }

    #[test]
    fn every_forbidden_tag_is_refused_whatever_its_case_or_spaces() {
        for tag in FORBIDDEN_TAGS {
            assert_eq!(check("t", &format!("{BODY}<{tag} x>")), Err(SceneError::Tag(tag)), "{tag}");
        }
        assert_eq!(check("t", "<SCRIPT>x</SCRIPT>"), Err(SceneError::Tag("script")));
        assert_eq!(check("t", "< script>x"), Err(SceneError::Tag("script")));
        assert_eq!(check("t", r#"<Meta http-equiv="refresh">"#), Err(SceneError::Tag("meta")));
        // A tag whose name only starts like one is not it.
        assert_eq!(check("t", "<formula></formula><scripted>"), Ok("t"));
    }

    #[test]
    fn an_event_handler_is_refused_and_a_word_with_on_in_it_is_not() {
        assert_eq!(check("t", r#"<div onclick="x()">"#), Err(SceneError::Handler("onclick".into())));
        assert_eq!(check("t", r#"<svg ONload = "x()">"#), Err(SceneError::Handler("onload".into())));
        assert_eq!(check("t", r#"<i class="a"onmouseover='x'>"#), Err(SceneError::Handler("onmouseover".into())));
        assert_eq!(check("t", "<svg/onload=x>"), Err(SceneError::Handler("onload".into())));
        assert_eq!(check("t", "<svg\nonload=x>"), Err(SceneError::Handler("onload".into())));
        assert_eq!(check("t", "<style>.a{transition:opacity 1s}.on{content:'on'}</style><i class=\"on\" data-on=\"1\" data-tone=\"1\">"), Ok("t"));
    }

    #[test]
    fn an_address_is_refused_but_the_svg_namespace_is_not() {
        assert_eq!(check("t", r#"<div style="background:url(https://x.test/a.png)">"#), Err(SceneError::Network));
        assert_eq!(check("t", r#"<img src="HTTP://x.test/a.png">"#), Err(SceneError::Network));
        assert_eq!(check("t", r#"<div style="background:url(http://www.w3.org/a.png)">"#), Err(SceneError::Network));
        assert_eq!(check("t", r#"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><image href="data:image/png;base64,AA"/></svg>"#), Ok("t"));
        assert_eq!(check("t", r#"<div xmlns="http://www.w3.org/1999/xhtml"></div>"#), Ok("t"));
    }

    #[test]
    fn the_fiftieth_scene_fits_and_the_fifty_first_does_not() {
        assert_eq!(room(MAX_SCENES - 1), Ok(()));
        assert_eq!(room(MAX_SCENES), Err(SceneError::Full));
    }

    #[test]
    fn an_id_is_a_plain_uuid_and_nothing_else() {
        let good = "6f1c1f0e-8d5b-4a3a-9a57-0b3c1f6d2e11";
        assert_eq!(id(good), Ok(good));
        for bad in ["../x", "", "6F1C1F0E-8D5B-4A3A-9A57-0B3C1F6D2E11", "{6f1c1f0e-8d5b-4a3a-9a57-0b3c1f6d2e11}", "6f1c1f0e8d5b4a3a9a570b3c1f6d2e11"] {
            assert_eq!(id(bad), Err(SceneError::BadId(bad.into())), "{bad}");
        }
    }
}
