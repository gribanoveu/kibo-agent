//! `paintScene` — a scene for behind Frost's glass, painted by the model
//! (`docs/55-frost-scenes.md`, decision 8): checked by `domain::scene`, kept
//! by `infra::scene_store`, and chosen by the window when the result arrives.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{PaintSceneArgs, ToolError, ToolResult};
use crate::infra::scene_store;

pub fn paint_scene(args: &PaintSceneArgs) -> Result<ToolResult, ToolError> {
    let kept = scene_store::add(&args.title, &args.html)?;
    Ok(ToolResult::ScenePainted { id: kept.id, title: kept.title })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "paintScene".to_string(),
        description: "Paint a moving scene for behind the app's window — a snowy forest, a night lake, frost on glass — when the user asks for one. \
It is kept and shown at once, under the window's frosted-glass panels, while the Frost theme is on; if the user is on another theme, \
say it shows under Frost, chosen in Settings → Appearance. To change a scene, paint it again whole: the old one stays in Settings until the user deletes it.

The scene is the inside of a page's <body>: a <style> block and elements. No <html> or <head>, and none of <script>, <meta>, \
<base>, <link>, <iframe>, <object>, <embed>, <form>, event handlers or http(s) addresses — it runs in a frame without scripts \
or network; pictures are CSS, inline SVG, or data: URLs in CSS and <img>.
- It fills the window: position elements absolutely, sizes in vw, vh, vmin or %; leave room for any window shape.
- Dark and quiet: the conversation is read over it. Deep colours, low contrast, no text. Keep the middle calmest — \
the chat sits there; put detail and motion toward the edges.
- Move only transform and opacity, with CSS @keyframes, slowly — tens of seconds a cycle. Never animate filter, sizes, \
colours, gradients or anything inside an <svg>: those redraw the window every frame. At most about a hundred moving elements.
- At time 0 it is already whole — it stands still at that frame when paused, and for users who ask for less motion. \
Spread repeated elements with negative animation-delay rather than having them arrive."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "The scene's name for its card in Settings, a few words: \"Snowy pine forest\"."
                },
                "html": {
                    "type": "string",
                    "description": "The scene: a <style> block and elements, the inside of <body>. At most 256 KB."
                }
            },
            "required": ["title", "html"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scene::SceneError;
    use crate::testing::with_app_dir;

    fn args(title: &str, html: &str) -> PaintSceneArgs {
        PaintSceneArgs { title: title.into(), html: html.into() }
    }

    #[test]
    fn a_good_scene_is_kept_and_named_by_its_id() {
        with_app_dir("paint-scene-good", || {
            let body = "<style>i{animation:a 20s infinite}@keyframes a{to{opacity:.5}}</style><i></i>";
            let Ok(ToolResult::ScenePainted { id, title }) = paint_scene(&args(" Night lake ", body)) else { panic!("not painted") };
            assert_eq!(title, "Night lake");
            assert_eq!(scene_store::read(&id), Ok(body.to_string()));
            assert_eq!(scene_store::list().unwrap().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
        });
    }

    #[test]
    fn a_bad_one_is_refused_in_the_domains_words_and_nothing_is_kept() {
        with_app_dir("paint-scene-bad", || {
            let refused = paint_scene(&args("Lake", "<i onclick=x()></i>")).unwrap_err();
            assert!(matches!(refused, ToolError::Scene(SceneError::Handler(_))), "{refused:?}");
            assert_eq!(refused.to_string(), SceneError::Handler("onclick".into()).to_string());
            assert_eq!(scene_store::list(), Ok(Vec::new()));
        });
    }
}
