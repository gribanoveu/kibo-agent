//! A user-facing error's translation code and the parameters its text takes
//! (`docs/65-interface-language.md`, LC-2). Pure: no serde, no framework —
//! the catalog (`src/locales/en.json`) holds the text for each code, and a
//! test holds the two in step. The boundary turns this into `UiError` in
//! LC-5; until then commands keep returning the `#[error]` string.

/// The code is `<area>.<variant>`, stable across releases: the catalog key.
/// Implemented with an exhaustive `match`, so a new variant without a code
/// does not compile.
pub trait UiText {
    fn code(&self) -> &'static str;
    /// The values the text's `{name}` placeholders take. An OS reason or a
    /// provider's own words go here untranslated — they are not the app's text.
    fn params(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}

/// One `reason` parameter — the common shape of a wrapped cause.
pub fn reason(value: impl ToString) -> Vec<(&'static str, String)> {
    vec![("reason", value.to_string())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn catalog() -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_str(include_str!("../../../src/locales/en.json")).unwrap()
    }

    /// The `{name}` placeholders of a catalog text, in order.
    fn placeholders(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            let end = after.find('}').expect("a catalog placeholder is closed");
            out.push(&after[..end]);
            rest = &after[end + 1..];
        }
        out
    }

    /// Every sample's code is a catalog key, its parameters are exactly the
    /// text's placeholders, and the text filled with them is the error's own
    /// `Display` — so a code swapped between variants of the same shape (both
    /// `{id}`) shows the wrong text and fails here. `internal` re-frames a
    /// non-user failure on purpose and is exempt.
    fn check<E: UiText + std::fmt::Display>(samples: Vec<E>, codes: &mut HashSet<&'static str>) {
        let catalog = catalog();
        for sample in samples {
            let code = sample.code();
            let text = catalog.get(code).unwrap_or_else(|| panic!("{code} is not in en.json"));
            let text = text.as_str().unwrap_or_else(|| panic!("{code} is not a string"));
            let params = sample.params();
            let mut asked: Vec<&str> = params.iter().map(|(name, _)| *name).collect();
            let mut written = placeholders(text);
            asked.sort_unstable();
            written.sort_unstable();
            assert_eq!(asked, written, "{code}: parameters and placeholders differ");
            if code != "internal" {
                let mut rendered = text.to_string();
                for (name, value) in &params {
                    rendered = rendered.replace(&format!("{{{name}}}"), value);
                }
                assert_eq!(rendered, sample.to_string(), "{code}: the catalog text is not the error's own");
            }
            codes.insert(code);
        }
    }

    /// All codes the domain produces, one sample per variant (`ui_samples`
    /// beside each enum; `architecture.rs` holds the samples complete).
    fn all_codes() -> HashSet<&'static str> {
        let mut codes = HashSet::new();
        check(crate::domain::settings::AppDirError::ui_samples(), &mut codes);
        check(crate::domain::settings::CredentialsError::ui_samples(), &mut codes);
        check(crate::domain::settings::SettingsError::ui_samples(), &mut codes);
        check(crate::domain::settings::RecentFoldersError::ui_samples(), &mut codes);
        check(crate::domain::workspace::WorkspaceError::ui_samples(), &mut codes);
        check(crate::domain::llm::LlmError::ui_samples(), &mut codes);
        check(crate::domain::mcp::McpConfigError::ui_samples(), &mut codes);
        check(crate::domain::mcp::McpError::ui_samples(), &mut codes);
        check(crate::infra::index_store::IndexStoreError::ui_samples(), &mut codes);
        check(crate::services::repo_index::RepoSyncError::ui_samples(), &mut codes);
        check(crate::services::workspace_index::WorkspaceIndexError::ui_samples(), &mut codes);
        check(crate::domain::turn::DecisionError::ui_samples(), &mut codes);
        check(crate::services::llm_chat::TurnError::ui_samples(), &mut codes);
        check(crate::services::turn_setup::TurnSetupError::ui_samples(), &mut codes);
        check(crate::domain::chat_record::ChatError::ui_samples(), &mut codes);
        codes
    }

    #[test]
    fn every_code_is_in_the_catalog_with_matching_parameters() {
        all_codes();
    }

    /// Parameters carry the data they wrap — including the pre-rendered
    /// fragments (the exit code, the HTTP hint), which no placeholder-name
    /// check would catch emptied.
    #[test]
    fn parameters_carry_the_data_they_wrap() {
        let e = crate::domain::workspace::WorkspaceError::CannotOpen { path: "/x".into(), reason: "gone".into() };
        assert_eq!(e.params(), vec![("path", "/x".to_string()), ("reason", "gone".to_string())]);
        let m = crate::domain::mcp::McpError::Exited { code: Some(3), stderr: "boom".into() };
        let params = m.params();
        assert_eq!(params[0], ("code", " with code 3".to_string()));
        assert!(params[1].1.contains("boom"), "{:?}", params[1]);
        let h = crate::domain::mcp::McpError::Http { status: 401, body: String::new() };
        assert!(h.params().iter().any(|(k, v)| *k == "hint" && v.contains("OAuth")), "{:?}", h.params());
    }

    /// A key no error produces is a leftover — the catalog and the domain
    /// say the same thing.
    #[test]
    fn the_catalog_has_no_orphan_keys() {
        let produced = all_codes();
        let catalog = catalog();
        let orphans: Vec<&String> = catalog.keys().filter(|key| !produced.contains(key.as_str())).collect();
        assert!(orphans.is_empty(), "catalog keys no UiText produces: {orphans:?}");
    }
}
