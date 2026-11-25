//! The provider and model a chat speaks with (`docs/64-chat-model.md`).
//!
//! One rule, written once: a chat with a pin speaks through it; a chat whose
//! provider was removed switches once to the active one and keeps that; a
//! chat without a pin — saved before pins existed — speaks through the active
//! provider, as every chat did before. The turn's resolution and the chat's
//! opening both ask here, so the chip and the turn say the same thing.

use crate::domain::chat_record::{ChatError, ChatModel};
use crate::domain::settings::LlmSettings;
use crate::infra::{chat_store, settings_store};

/// The chat's effective pin, or `None` when it has none.
///
/// A pin whose provider is gone is replaced by the active provider and its
/// model — and written into the chat, so the substitution happens once: a
/// provider added back under the same id does not pull the chat back to a
/// choice the user has already seen replaced. With no provider left at all
/// there is nothing to switch to; the stored pin stays for a later read.
pub fn pin(chat: &str, llm: &LlmSettings) -> Result<Option<ChatModel>, ChatError> {
    let Some(pin) = chat_store::model(chat)? else { return Ok(None) };
    if llm.provider(&pin.provider).is_some() {
        return Ok(Some(pin));
    }
    let Some(active) = llm.active() else { return Ok(None) };
    let substitute = ChatModel { provider: active.id.clone(), model: active.model.clone() };
    // Conditional on the pin still naming the removed provider: a pick made
    // since the read above stands, and is what the chat speaks through.
    if chat_store::replace_model(chat, &pin.provider, &substitute)? {
        Ok(Some(substitute))
    } else {
        chat_store::model(chat)
    }
}

/// Pins the chat to the provider a new conversation starts on — the active
/// one and its model, what the chip showed a moment ago. A save calls it;
/// the store's `WHERE model_provider IS NULL` makes a repeated save a no-op
/// and keeps a pick made in the chat. Nothing configured, nothing stamped:
/// the chat stays unpinned and speaks through whatever is active later.
pub fn stamp_if_absent(chat: &str) -> Result<(), ChatError> {
    let llm = settings_store::load().map_err(|e| ChatError::Store(e.to_string()))?.llm;
    let Some(active) = llm.active() else { return Ok(()) };
    chat_store::stamp_model_if_absent(chat, &ChatModel { provider: active.id.clone(), model: active.model.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::{AppSettings, ProviderConfig};
    use crate::infra::chat_store;
    use crate::testing::with_app_dir;

    fn llm(providers: &[(&str, Option<&str>)], active: Option<&str>) -> LlmSettings {
        LlmSettings {
            active_provider_id: active.map(str::to_string),
            providers: providers
                .iter()
                .map(|(id, model)| ProviderConfig {
                    id: id.to_string(),
                    base_url: format!("https://{id}.example/v1"),
                    model: model.map(str::to_string),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn saved(id: &str) {
        chat_store::save(id, "/repo", &[], &serde_json::json!([]), &[], None, None, None).unwrap();
    }

    fn pin_of(provider: &str, model: Option<&str>) -> ChatModel {
        ChatModel { provider: provider.to_string(), model: model.map(str::to_string) }
    }

    /// The substitution is written, not computed each time: a provider added
    /// back under the same id must not pull the chat back to it.
    #[test]
    fn a_removed_provider_is_substituted_once_and_written() {
        with_app_dir("chat-model-substitute", || {
            saved("c1");
            chat_store::set_model("c1", &pin_of("gone", Some("m"))).unwrap();
            let settings = llm(&[("local", Some("qwen"))], Some("local"));

            let effective = pin("c1", &settings).unwrap();

            assert_eq!(effective, Some(pin_of("local", Some("qwen"))));
            assert_eq!(chat_store::model("c1").unwrap(), Some(pin_of("local", Some("qwen"))), "written into the chat");
            // The old id comes back — the chat stays where it was moved.
            let returned = llm(&[("local", Some("qwen")), ("gone", Some("m"))], Some("local"));
            assert_eq!(pin("c1", &returned).unwrap(), Some(pin_of("local", Some("qwen"))));
        });
    }

    /// A pin whose provider stands is returned as it is — auto included —
    /// and nothing is rewritten.
    #[test]
    fn a_live_pin_stands_as_it_is() {
        with_app_dir("chat-model-live", || {
            saved("c1");
            chat_store::set_model("c1", &pin_of("local", None)).unwrap();
            let settings = llm(&[("local", Some("qwen")), ("other", Some("gpt"))], Some("other"));

            assert_eq!(pin("c1", &settings).unwrap(), Some(pin_of("local", None)), "auto follows its own provider, not the active one");
            assert_eq!(chat_store::model("c1").unwrap(), Some(pin_of("local", None)));
        });
    }

    /// An unpinned chat has no pair of its own; nothing is invented for it.
    #[test]
    fn an_unpinned_chat_has_no_pin() {
        with_app_dir("chat-model-unpinned", || {
            saved("c1");
            let settings = llm(&[("local", Some("qwen"))], Some("local"));

            assert_eq!(pin("c1", &settings).unwrap(), None);
            assert_eq!(chat_store::model("c1").unwrap(), None, "reading does not stamp");
        });
    }

    /// Every provider gone: nothing to switch to, the stored pin stays for a
    /// later read, and the turn's resolution reports "no provider" itself.
    #[test]
    fn nothing_configured_leaves_the_stored_pin() {
        with_app_dir("chat-model-empty", || {
            saved("c1");
            chat_store::set_model("c1", &pin_of("gone", Some("m"))).unwrap();

            assert_eq!(pin("c1", &llm(&[], None)).unwrap(), None);
            assert_eq!(chat_store::model("c1").unwrap(), Some(pin_of("gone", Some("m"))));
        });
    }

    /// The stamp is the active provider and its model; a second stamp, under
    /// another active provider, does not move the chat.
    #[test]
    fn a_stamp_pins_the_active_once() {
        with_app_dir("chat-model-stamp", || {
            // The active one is not the first in the list: the stamp is the
            // active provider, not whichever happens to lead.
            settings_store::save(&AppSettings { llm: llm(&[("a", Some("ma")), ("b", Some("mb"))], Some("b")), ..Default::default() }).unwrap();
            saved("c1");

            stamp_if_absent("c1").unwrap();
            assert_eq!(chat_store::model("c1").unwrap(), Some(pin_of("b", Some("mb"))));

            settings_store::save(&AppSettings { llm: llm(&[("a", Some("ma")), ("b", Some("mb"))], Some("a")), ..Default::default() }).unwrap();
            stamp_if_absent("c1").unwrap();
            assert_eq!(chat_store::model("c1").unwrap(), Some(pin_of("b", Some("mb"))), "the first stamp stands");
        });
    }

    /// Nothing configured: nothing stamped, and no error — the chat opens
    /// and saves as it did before providers existed.
    #[test]
    fn nothing_configured_stamps_nothing() {
        with_app_dir("chat-model-stamp-empty", || {
            saved("c1");
            stamp_if_absent("c1").unwrap();
            assert_eq!(chat_store::model("c1").unwrap(), None);
        });
    }
}
