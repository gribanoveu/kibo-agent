//! The MongoDB feature's tools for the model — designed, not yet built
//! (`docs/43-mongodb.md`, "Инструменты агента"). Nothing here is offered: none
//! of these is a `ToolName` yet, so no role or mode can list one, and the
//! feature's manifest names no tools.
//!
//! What is settled is their shape: the names the model will call, which of
//! them change data (and so go through the approval card), what each takes,
//! and where they work — a connection and a database pinned to the chat, as
//! the Kubernetes role pins a namespace, so the model names a collection and
//! never a server. Building one is the contract in AGENTS.md: a `ToolName`
//! variant with this wire name, a `ToolCall` holding these args, a module
//! under `services/ai_tools/tools/` calling `services::mongo` (the page's own
//! service), and a row in the feature's manifest.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MongoTool {
    Databases,
    Collections,
    Find,
    Count,
    Aggregate,
    Indexes,
    Schema,
    Insert,
    Update,
    Delete,
}

impl MongoTool {
    pub const ALL: &'static [MongoTool] = &[
        MongoTool::Databases,
        MongoTool::Collections,
        MongoTool::Find,
        MongoTool::Count,
        MongoTool::Aggregate,
        MongoTool::Indexes,
        MongoTool::Schema,
        MongoTool::Insert,
        MongoTool::Update,
        MongoTool::Delete,
    ];

    /// What the model will call it.
    pub fn wire_name(self) -> &'static str {
        match self {
            MongoTool::Databases => "mongoDatabases",
            MongoTool::Collections => "mongoCollections",
            MongoTool::Find => "mongoFind",
            MongoTool::Count => "mongoCount",
            MongoTool::Aggregate => "mongoAggregate",
            MongoTool::Indexes => "mongoIndexes",
            MongoTool::Schema => "mongoSchema",
            MongoTool::Insert => "mongoInsert",
            MongoTool::Update => "mongoUpdate",
            MongoTool::Delete => "mongoDelete",
        }
    }

    /// Changes data: asks on the approval card, and is never a helper's or
    /// Plan's or Ask's — `ToolName::is_mutating` will say the same.
    pub fn is_mutating(self) -> bool {
        match self {
            MongoTool::Insert | MongoTool::Update | MongoTool::Delete => true,
            MongoTool::Databases
            | MongoTool::Collections
            | MongoTool::Find
            | MongoTool::Count
            | MongoTool::Aggregate
            | MongoTool::Indexes
            | MongoTool::Schema => false,
        }
    }

    /// What it is for, as the design says it.
    pub fn purpose(self) -> &'static str {
        match self {
            MongoTool::Databases => "The pinned connection's databases, with their sizes.",
            MongoTool::Collections => "The pinned database's collections and views.",
            MongoTool::Find => "Documents matching a filter, with a projection, a sort and a limit.",
            MongoTool::Count => "How many documents match a filter.",
            MongoTool::Aggregate => "An aggregation pipeline's results; $out and $merge are refused — they write.",
            MongoTool::Indexes => "A collection's indexes: keys, unique, TTL, partial filter.",
            MongoTool::Schema => "The fields a sample of documents has, with their types and how often each appears.",
            MongoTool::Insert => "One document or several into a collection.",
            MongoTool::Update => "An update to the documents matching a filter — one, or many when asked.",
            MongoTool::Delete => "The documents matching a filter — one, or many when asked; an empty filter is refused.",
        }
    }
}

/// Where the tools work: chosen by the user for the chat, never by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MongoPin {
    /// A saved connection's id (`infra::mongo_store`).
    pub connection: String,
    pub database: String,
}

/// At most this many documents go to the model from one call, each written
/// as mongosh shows it ([`super::mongo::to_shell`]) and cut past [`MAX_DOCUMENT_CHARS`].
pub const MAX_DOCUMENTS: usize = 50;
pub const MAX_DOCUMENT_CHARS: usize = 4_000;
/// What `mongoSchema` samples.
pub const SCHEMA_SAMPLE: usize = 100;

// The arguments, as the model will send them. Query text is mongosh's — what
// the page's fields take (`super::mongo::parse`) — so a filter the model
// writes can be pasted into the page and back.

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionArgs {
    pub collection: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FindArgs {
    pub collection: String,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub projection: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub skip: Option<u64>,
    /// Up to [`MAX_DOCUMENTS`].
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CountArgs {
    pub collection: String,
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AggregateArgs {
    pub collection: String,
    pub pipeline: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InsertArgs {
    pub collection: String,
    /// One document, or an array of them.
    pub documents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateArgs {
    pub collection: String,
    pub filter: String,
    /// An update document (`{ $set: … }`) or a pipeline.
    pub update: String,
    /// Every match rather than the first.
    #[serde(default)]
    pub many: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteArgs {
    pub collection: String,
    pub filter: String,
    #[serde(default)]
    pub many: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::ToolName;

    #[test]
    fn the_names_are_unique_and_none_is_taken() {
        let mut names: Vec<&str> = MongoTool::ALL.iter().map(|t| t.wire_name()).collect();
        assert!(names.iter().all(|n| n.starts_with("mongo")), "{names:?}");
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), MongoTool::ALL.len());
        for tool in ToolName::ALL {
            assert!(!names.contains(&tool.wire_name()), "{} is a tool already", tool.wire_name());
        }
    }

    #[test]
    fn only_the_writes_ask() {
        let mutating: Vec<_> = MongoTool::ALL.iter().filter(|t| t.is_mutating()).map(|t| t.wire_name()).collect();
        assert_eq!(mutating, ["mongoInsert", "mongoUpdate", "mongoDelete"]);
    }

    #[test]
    fn the_args_read_as_the_model_will_send_them() {
        let find: FindArgs = serde_json::from_str(r#"{"collection":"users","filter":"{ age: { $gt: 30 } }","limit":5}"#).unwrap();
        assert_eq!((find.collection.as_str(), find.limit, find.sort), ("users", Some(5), None));
        let delete: DeleteArgs = serde_json::from_str(r#"{"collection":"users","filter":"{ _id: 1 }"}"#).unwrap();
        assert!(!delete.many, "one document unless asked");
        assert!(serde_json::from_str::<CountArgs>(r#"{"collection":"u","database":"other"}"#).is_err(), "the database is the chat's, not the model's");
    }
}
