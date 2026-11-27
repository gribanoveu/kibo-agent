//! Product metrics: what the app records about its own use, in the shape the
//! analytics table takes them (`docs/67-app-metrics.md`).
//!
//! One variant per event, as `domain::metrics` has one per counter: the
//! category, the machine name, the human label and the slots an event fills
//! are stated here once, so a new metric is a variant and a call, and a label
//! cannot drift from the name it belongs to. A row carries the OS and the
//! build as they were when it happened, never as they are when it is sent.

use serde::Serialize;

/// What `event.type` holds for every row: the analytics table's name for a
/// metric of the app itself.
pub const EVENT_TYPE: &str = "app_metric";

/// How many slots the table has: `dimension.1` through `dimension.20`.
pub const MAX_DIMENSIONS: usize = 20;

/// How long a slot's value may be. Longer is not one of the names a slot is
/// for — a model's, a tool's, a mode's — and is cut rather than sent.
pub const MAX_DIMENSION_CHARS: usize = 64;

/// What a slot holds instead of a value that looks like content.
pub const REDACTED: &str = "<redacted>";

/// What the event is about, in a few words — `event.category`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// The app itself: a launch, the first run, an update.
    App,
    /// A conversation: begun, resumed, exported.
    Chat,
    /// One turn of a conversation.
    Turn,
    /// A tool the model called.
    Tool,
    /// A feature turned on or off from the Catalog.
    Feature,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::App => "app",
            Category::Chat => "chat",
            Category::Turn => "turn",
            Category::Tool => "tool",
            Category::Feature => "feature",
        }
    }
}

/// The meaning of each `dimension.N` the app may fill: the slot's number in
/// the table and the name it is known by here.
///
/// A slot means the same thing in every event, because the table reads the
/// number and nothing else: `dimension.3` that is a model in one row and a
/// tool in the next is a column that cannot be grouped. A new kind of value
/// takes the next free number rather than a meaning someone else's rows
/// already carry.
pub const SLOTS: &[(&str, u8)] = &[
    ("mode", 1),
    ("role", 2),
    ("provider", 3),
    ("model", 4),
    ("tool", 5),
    ("feature", 6),
    ("outcome", 7),
    ("rounds", 8),
    ("tokens", 9),
    ("error", 10),
];

/// The slot `name` fills; `None` for a name no slot has.
pub fn slot(name: &str) -> Option<u8> {
    SLOTS.iter().find(|(known, _)| *known == name).map(|(_, number)| *number)
}

/// One event the app records. A variant carries what it is about; everything
/// a row needs besides that is the same for every event and is stamped by
/// [`Event::of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEvent {
    /// The window is up — once per launch, in whatever mode the app opens.
    AppLaunched,
}

impl ProductEvent {
    /// Every event there is, for the tests that pin their names.
    pub const ALL: &'static [ProductEvent] = &[ProductEvent::AppLaunched];

    pub fn category(self) -> Category {
        match self {
            ProductEvent::AppLaunched => Category::App,
        }
    }

    /// `event.name` — `category.something`, never changed once it is sent.
    pub fn name(self) -> &'static str {
        match self {
            ProductEvent::AppLaunched => "app.launch",
        }
    }

    /// `event.label` — what the event is, for a person reading the table.
    pub fn label(self) -> &'static str {
        match self {
            ProductEvent::AppLaunched => "App launched",
        }
    }

    /// The slots this event fills, by name, in any order: [`Event::of`] sorts
    /// them into the table's numbers.
    pub fn dimensions(self) -> Vec<(&'static str, String)> {
        match self {
            ProductEvent::AppLaunched => Vec::new(),
        }
    }
}

/// What a row needs beyond its event: who and when, and on what. Filled by
/// the store, so that nothing but `infra` reads the clock or the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// `id.event` — unique per row, so a delivery repeated after a failure is
    /// not a second event.
    pub id: String,
    /// `occurred_at` — the moment it happened, UTC with milliseconds.
    pub occurred_at: String,
    pub os_name: String,
    pub os_version: String,
    pub os_arch: String,
    /// The version of the build that recorded it.
    pub app_version: &'static str,
    /// `debug` or `release`: what a developer's `tauri dev` records is not
    /// what a user's install does.
    pub app_build: &'static str,
}

/// One `dimension.N`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dimension {
    /// Its number in the table, 1–20.
    pub number: u8,
    pub name: &'static str,
    pub value: String,
}

/// One row, as it is stored and as it will be sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub occurred_at: String,
    pub category: &'static str,
    pub name: &'static str,
    pub label: &'static str,
    pub event_type: &'static str,
    /// By slot number, so the table's columns come out in its own order.
    pub dimensions: Vec<Dimension>,
    pub os_name: String,
    pub os_version: String,
    pub os_arch: String,
    pub app_version: &'static str,
    pub app_build: &'static str,
}

impl Event {
    pub fn of(event: ProductEvent, stamp: Stamp) -> Event {
        Event {
            id: stamp.id,
            occurred_at: stamp.occurred_at,
            category: event.category().as_str(),
            name: event.name(),
            label: event.label(),
            event_type: EVENT_TYPE,
            dimensions: placed(event.dimensions()),
            os_name: stamp.os_name,
            os_version: stamp.os_version,
            os_arch: stamp.os_arch,
            app_version: stamp.app_version,
            app_build: stamp.app_build,
        }
    }
}

/// The slots of a row: known ones only, redacted, in the table's own order.
/// There can never be more than the table has — [`SLOTS`] is shorter than
/// [`MAX_DIMENSIONS`] by the same test that keeps every number inside it.
///
/// A name no slot has is a mistake in the event, not a value to send: it is
/// left out rather than written under number 0.
pub fn placed(dimensions: Vec<(&'static str, String)>) -> Vec<Dimension> {
    let mut placed: Vec<Dimension> = dimensions
        .into_iter()
        .filter_map(|(name, value)| Some(Dimension { number: slot(name)?, name, value: redacted(&value) }))
        .collect();
    placed.sort_by_key(|dimension| dimension.number);
    placed
}

/// A slot's value, or [`REDACTED`] where it looks like something the policy
/// keeps on this machine: a path, more than one line, or too long to be one
/// of the short names a slot is for.
///
/// This is a heuristic, as `kube_view::redact` is, and it is not a filter for
/// what a caller decides to pass: a model's name or a tool's passes, a folder
/// or a file does not.
pub fn redacted(value: &str) -> String {
    let content_like = value.contains('/') || value.contains('\\') || value.contains(['\n', '\r']);
    if content_like || value.chars().count() > MAX_DIMENSION_CHARS {
        REDACTED.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> Stamp {
        Stamp {
            id: "e1".into(),
            occurred_at: "2026-11-27T10:00:00.000Z".into(),
            os_name: "macOS".into(),
            os_version: "15.6".into(),
            os_arch: "aarch64".into(),
            app_version: "0.14.0",
            app_build: "debug",
        }
    }

    /// The table reads numbers, not names: two slots on one number, or two
    /// names on one slot, is a column nobody can interpret.
    #[test]
    fn every_slot_has_its_own_number_and_name() {
        for (name, number) in SLOTS {
            assert!(!name.is_empty(), "a slot with no name");
            assert!((1..=MAX_DIMENSIONS as u8).contains(number), "{name} is outside the table: {number}");
            assert_eq!(SLOTS.iter().filter(|(_, n)| n == number).count(), 1, "two slots on dimension.{number}");
            assert_eq!(SLOTS.iter().filter(|(n, _)| n == name).count(), 1, "two slots named {name}");
            assert_eq!(slot(name), Some(*number));
        }
        assert_eq!(slot("nothing"), None);
    }

    /// A name is forever: rows already sent under it would stop adding up.
    #[test]
    fn every_event_has_one_name_one_label_and_its_category() {
        let mut names = Vec::new();
        for event in ProductEvent::ALL {
            let name = event.name();
            assert!(name.starts_with(event.category().as_str()), "{name} is not under its category");
            assert!(name.len() > event.category().as_str().len() + 1, "{name} names nothing");
            assert!(!event.label().is_empty(), "{name} has no label");
            assert!(!names.contains(&name), "{name} is used twice");
            names.push(name);
        }
        assert_eq!(names, ["app.launch"], "a name changed: rows under the old one are orphaned");
    }

    /// What a launch fills: nothing, and every mandatory field of the row.
    #[test]
    fn a_row_is_stamped_with_what_every_event_shares() {
        let row = Event::of(ProductEvent::AppLaunched, stamp());
        assert_eq!(
            (row.category, row.name, row.label, row.event_type),
            ("app", "app.launch", "App launched", "app_metric")
        );
        assert!(row.dimensions.is_empty());
        assert_eq!(
            (row.os_name.as_str(), row.os_version.as_str(), row.os_arch.as_str(), row.app_version, row.app_build),
            ("macOS", "15.6", "aarch64", "0.14.0", "debug")
        );
        assert_eq!((row.id.as_str(), row.occurred_at.as_str()), ("e1", "2026-11-27T10:00:00.000Z"));
    }

    #[test]
    fn slots_come_out_in_the_tables_order_and_unknown_names_are_dropped() {
        let dimensions = placed(vec![
            ("tool", "readFile".to_string()),
            ("mode", "agent".to_string()),
            ("unknown", "x".to_string()),
            ("model", "/Users/me/notes".to_string()),
        ]);
        assert_eq!(
            dimensions,
            [
                Dimension { number: 1, name: "mode", value: "agent".into() },
                Dimension { number: 4, name: "model", value: REDACTED.into() },
                Dimension { number: 5, name: "tool", value: "readFile".into() },
            ]
        );
    }

    /// An event naming a slot the table does not have loses it without a word,
    /// and a slot named twice writes two values into one column. Both are found
    /// here — for the events of the next stage too, this being over `ALL` —
    /// rather than in a column of somebody else's table that stays empty.
    #[test]
    fn every_events_slots_are_known_and_named_once() {
        for event in ProductEvent::ALL {
            let mut names = Vec::new();
            for (name, _) in event.dimensions() {
                assert!(slot(name).is_some(), "{} fills a slot no table has: {name}", event.name());
                assert!(!names.contains(&name), "{} names {name} twice", event.name());
                names.push(name);
            }
        }
    }

    #[test]
    fn a_value_that_looks_like_a_path_or_runs_long_is_cut() {
        assert_eq!(redacted("agent"), "agent");
        assert_eq!(redacted("qwen3-coder-480b-a35b-instruct"), "qwen3-coder-480b-a35b-instruct");
        assert_eq!(redacted("/Users/me/work/repo"), REDACTED);
        assert_eq!(redacted(r"C:\work\repo"), REDACTED);
        assert_eq!(redacted("first\nsecond"), REDACTED);
        assert_eq!(redacted("first\rsecond"), REDACTED);
        assert_eq!(redacted(&"a".repeat(MAX_DIMENSION_CHARS)), "a".repeat(MAX_DIMENSION_CHARS));
        assert_eq!(redacted(&"a".repeat(MAX_DIMENSION_CHARS + 1)), REDACTED);
        // Counted in characters, not bytes: a Cyrillic name is not long.
        assert_eq!(redacted(&"м".repeat(MAX_DIMENSION_CHARS)), "м".repeat(MAX_DIMENSION_CHARS));
    }
}
