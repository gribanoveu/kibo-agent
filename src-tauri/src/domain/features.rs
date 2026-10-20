//! Features the user turns on from the Catalog page (`docs/33-features.md`):
//! code built into the app that stays out of the model's sight and the
//! window's until chosen. A feature owns tools — the roles and modes list them
//! as usual, and this filters them out while it is off — and the window's
//! pages carry its id.
//!
//! Adding one is a variant here, a row in [`Feature::manifest`], and
//! `feature: "<id>"` on its page in `components/pages.tsx`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Feature {
    Board,
    ProviderBalance,
    DeepseekSearch,
    MeetingNotes,
    VoiceMemory,
}

/// What a feature is and what it adds, for the filters and the Catalog's card.
pub struct Manifest {
    /// `[a-z0-9-]`: its key in `settings.json` and in the window.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub enabled_by_default: bool,
    pub tools: &'static [ToolName],
    /// Its file or folder under the app directory, which "Delete data" removes.
    pub data: Option<&'static str>,
}

impl Feature {
    pub const ALL: &'static [Feature] = &[Feature::Board, Feature::ProviderBalance, Feature::DeepseekSearch, Feature::MeetingNotes, Feature::VoiceMemory];

    pub fn manifest(self) -> Manifest {
        match self {
            Feature::Board => Manifest {
                id: "board",
                name: "Board",
                description: "Cards for later: ask the model to remind you of something and it lands on the Board page, \
                    in To do, Doing and Done.",
                enabled_by_default: false,
                tools: &[ToolName::Board],
                data: Some("board.json"),
            },
            Feature::ProviderBalance => Manifest {
                id: "provider-balance",
                name: "Provider balance",
                description: "What is left on the account, beside the model under the message box — for DeepSeek, \
                    known by its address api.deepseek.com. Read again after each answer.",
                enabled_by_default: false,
                tools: &[],
                data: None,
            },
            Feature::DeepseekSearch => Manifest {
                id: "deepseek-search",
                name: "DeepSeek search",
                description: "Two tools for the model, deepseekSearch and deepseekFetch: DeepSeek's own web search and page reader, \
                    paid from the DeepSeek account set up in Settings → Models. Apart from the web search chosen in Settings.",
                enabled_by_default: false,
                tools: &[ToolName::DeepseekSearch, ToolName::DeepseekFetch],
                data: None,
            },
            Feature::MeetingNotes => Manifest {
                id: "meeting-notes",
                name: "Meeting notes",
                description: "A Meetings page: add a call's recording and get its notes — summary, decisions, action items — \
                    written by the model from a transcript with speakers and times. The speech is recognized on this computer; \
                    only the transcript goes to the model chosen in Settings.",
                enabled_by_default: false,
                tools: &[],
                data: Some("meetings"),
            },
            Feature::VoiceMemory => Manifest {
                id: "voice-memory",
                name: "Voice memory",
                description: "With Meeting notes: a speaker you name on the Meetings page is named by voice in the next meeting. \
                    It keeps a voice print of each speaker who said 20 seconds and 20 words or more in phrases in a meeting — biometric data about the people on your \
                    calls, so have their consent. Kept on this computer only; Delete data removes every print.",
                enabled_by_default: false,
                tools: &[],
                data: Some("voices.json"),
            },
        }
    }

    pub fn from_id(id: &str) -> Option<Feature> {
        Feature::ALL.iter().copied().find(|f| f.manifest().id == id)
    }

    /// The feature `tool` belongs to; `None` for a tool that is always there.
    pub fn owning(tool: ToolName) -> Option<Feature> {
        Feature::ALL.iter().copied().find(|f| f.manifest().tools.contains(&tool))
    }
}

/// Which features are on — read once a turn, as skills and MCP servers are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features {
    on: Vec<Feature>,
}

impl Features {
    /// The user's choices, by id; a feature not chosen is its default. An id
    /// this build does not know is skipped — and kept in the file, so a newer
    /// build's choice outlives a run of this one.
    pub fn from_settings(chosen: &BTreeMap<String, bool>) -> Self {
        let on = Feature::ALL
            .iter()
            .copied()
            .filter(|f| {
                let manifest = f.manifest();
                chosen.get(manifest.id).copied().unwrap_or(manifest.enabled_by_default)
            })
            .collect();
        Self { on }
    }

    pub fn all() -> Self {
        Self { on: Feature::ALL.to_vec() }
    }

    pub fn is_on(&self, feature: Feature) -> bool {
        self.on.contains(&feature)
    }

    /// Whether `tool` may be offered: always, unless a feature that is off owns it.
    pub fn offers(&self, tool: ToolName) -> bool {
        Feature::owning(tool).is_none_or(|f| self.is_on(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_found_again() {
        for &feature in Feature::ALL {
            let id = feature.manifest().id;
            assert!(id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{id}");
            assert_eq!(Feature::from_id(id), Some(feature));
        }
        assert_eq!(Feature::from_id("nope"), None);
    }

    /// `src/lib/features.ts` names them by these ids.
    #[test]
    fn the_ids_are_the_ones_the_window_knows() {
        let ids: Vec<&str> = Feature::ALL.iter().map(|f| f.manifest().id).collect();
        assert_eq!(ids, ["board", "provider-balance", "deepseek-search", "meeting-notes", "voice-memory"]);
    }

    #[test]
    fn a_tool_belongs_to_one_feature_at_most() {
        for &tool in ToolName::ALL {
            let owners = Feature::ALL.iter().filter(|f| f.manifest().tools.contains(&tool)).count();
            assert!(owners <= 1, "{tool:?}");
        }
    }

    #[test]
    fn a_feature_not_chosen_is_its_default_and_a_choice_wins_either_way() {
        let chosen = |pairs: &[(&str, bool)]| pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect::<BTreeMap<_, _>>();
        assert!(!Features::from_settings(&chosen(&[])).is_on(Feature::Board), "the board is off until chosen");
        assert!(!Features::from_settings(&chosen(&[])).is_on(Feature::ProviderBalance), "a live call per answer, off until chosen");
        assert!(!Features::from_settings(&chosen(&[])).is_on(Feature::DeepseekSearch), "paid per call, off until chosen");
        assert!(!Features::from_settings(&chosen(&[])).is_on(Feature::MeetingNotes), "experimental, off until chosen");
        assert!(!Features::from_settings(&chosen(&[])).is_on(Feature::VoiceMemory), "voice prints, off until chosen");
        assert!(Features::from_settings(&chosen(&[("board", true)])).is_on(Feature::Board));
        assert!(!Features::from_settings(&chosen(&[("board", false)])).is_on(Feature::Board));
        // An id from another build changes nothing here.
        assert_eq!(Features::from_settings(&chosen(&[("timer", true)])), Features::default());
    }

    #[test]
    fn a_tool_of_a_feature_that_is_off_is_not_offered_and_every_other_tool_is() {
        let off = Features::default();
        let on = Features::all();
        for &tool in ToolName::ALL {
            let owned = Feature::owning(tool).is_some();
            assert_eq!(off.offers(tool), !owned, "{tool:?}");
            assert!(on.offers(tool), "{tool:?}");
        }
        assert!(!off.offers(ToolName::Board));
    }
}
