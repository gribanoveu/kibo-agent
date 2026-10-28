//! `mongo` — the MongoDB connections the user opened to the agent, read
//! (`docs/44-mongodb.md`, M-8). One tool with an `op`, as `board`: where a
//! collection is, a sample's fields, a find, a count, an aggregation, the
//! indexes. Nothing is written — a pipeline with `$out` or `$merge` is
//! refused before the server is asked.
//!
//! The model names a connection by the name the user gave it, never by its
//! URI, and sees only the ones ticked "Open to the agent". What it reads is
//! redacted (`domain::mongo::redact_document`) and cut to size.

use serde_json::{json, Map, Value};

use crate::domain::llm::LlmToolDefinition;
use crate::domain::mongo::{self, ConnectionView, FindSpec, IndexView, MongoError, MongoServers, Target};
use crate::domain::tools::{MongoArgs, MongoOp, ToolDeps, ToolError, ToolResult};

/// Most documents a find or an aggregation hands the model.
const MOST: usize = 50;
/// A find's, when `limit` is not given.
const FIND_DEFAULT: usize = 20;
/// The longest a document is shown; past it, cut.
const DOCUMENT_CHARS: usize = 4000;
/// How many documents `schema` samples.
const SAMPLE: usize = 100;
/// Most fields `schema` lists.
const FIELDS: usize = 200;
/// Most collections `collections` lists; the rest are counted.
const LISTED: usize = 50;
/// The server's own: listed and searched only when named.
const SYSTEM_DATABASES: &[&str] = &["admin", "config", "local"];

pub fn mongo(args: &MongoArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let servers = deps.mongo.as_deref().ok_or(MongoError::NoneOpen)?;
    let open: Vec<ConnectionView> = servers.connections()?.into_iter().filter(|c| c.agent).collect();
    if open.is_empty() {
        return Err(MongoError::NoneOpen.into());
    }
    if args.op == MongoOp::Collections {
        let (text, summary) = collections(servers, &open, args)?;
        return Ok(ToolResult::Mongo { text, summary });
    }
    let (connection, target) = target(&open, args)?;
    let at = format!("{}.{} on {}", target.database, target.collection, connection.name);
    let (text, summary) = match args.op {
        // The connections and databases, answered above.
        MongoOp::Collections => collections(servers, &open, args)?,
        MongoOp::Schema => schema(servers, &target, &at)?,
        MongoOp::Find => find(servers, &target, &at, args)?,
        MongoOp::Count => {
            let count = servers.count(&target, query(&args.filter, "the filter")?)?;
            (format!("{count} documents in {at} match."), format!("{count} documents"))
        }
        MongoOp::Aggregate => aggregate(servers, &target, &at, args)?,
        MongoOp::Indexes => indexes(&servers.indexes(&target)?, &at),
    };
    Ok(ToolResult::Mongo { text, summary })
}

fn missing(op: MongoOp, field: &str) -> ToolError {
    ToolError::InvalidArguments { tool: "mongo".into(), reason: format!("op \"{}\" needs `{field}`", op_name(op)) }
}

fn op_name(op: MongoOp) -> &'static str {
    match op {
        MongoOp::Collections => "collections",
        MongoOp::Schema => "schema",
        MongoOp::Find => "find",
        MongoOp::Count => "count",
        MongoOp::Aggregate => "aggregate",
        MongoOp::Indexes => "indexes",
    }
}

fn names(open: &[ConnectionView]) -> String {
    open.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", ")
}

/// The connection by its name, any case; with none named, the only one open.
fn connection<'a>(open: &'a [ConnectionView], name: Option<&str>, op: MongoOp) -> Result<&'a ConnectionView, ToolError> {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return match open {
            [only] => Ok(only),
            _ => Err(ToolError::InvalidArguments {
                tool: "mongo".into(),
                reason: format!("op \"{}\" needs `connection`, one of {}", op_name(op), names(open)),
            }),
        };
    };
    let named: Vec<&ConnectionView> = open.iter().filter(|c| c.name.to_lowercase() == name.to_lowercase()).collect();
    match named[..] {
        [one] => Ok(one),
        [] => Err(MongoError::NotOpen { name: name.into(), open: names(open) }.into()),
        _ => Err(MongoError::SameName { name: name.into(), count: named.len() }.into()),
    }
}

fn target<'a>(open: &'a [ConnectionView], args: &MongoArgs) -> Result<(&'a ConnectionView, Target), ToolError> {
    let given = |value: &Option<String>, field: &str| {
        value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).ok_or_else(|| missing(args.op, field))
    };
    let connection = connection(open, args.connection.as_deref(), args.op)?;
    let target = Target { connection: connection.id.clone(), database: given(&args.database, "database")?, collection: given(&args.collection, "collection")? };
    Ok((connection, target))
}

fn query(text: &Option<String>, what: &'static str) -> Result<Map<String, Value>, MongoError> {
    mongo::parse_document(text.as_deref().unwrap_or_default(), what)
}

/// Without `name` or `database`, each connection's databases; with either,
/// the collections there, those whose name has `name` in it.
fn collections(servers: &dyn MongoServers, open: &[ConnectionView], args: &MongoArgs) -> Result<(String, String), ToolError> {
    let connections = match &args.connection {
        Some(_) => vec![connection(open, args.connection.as_deref(), args.op)?],
        None => open.iter().collect(),
    };
    let database = args.database.as_deref().map(str::trim).filter(|d| !d.is_empty());
    let needle = args.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
    let databases_of = |connection: &ConnectionView| -> Result<Vec<String>, MongoError> {
        match database {
            Some(database) => Ok(vec![database.to_string()]),
            None => Ok(servers.databases(&connection.id)?.into_iter().map(|d| d.name).filter(|d| !SYSTEM_DATABASES.contains(&d.as_str())).collect()),
        }
    };

    if needle.is_none() && database.is_none() {
        let lines: Vec<String> = connections
            .iter()
            .map(|connection| match databases_of(connection) {
                Ok(databases) if databases.is_empty() => format!("- {}: no databases", connection.name),
                Ok(databases) => format!("- {}: {}", connection.name, databases.join(", ")),
                Err(error) => format!("- {}: did not answer — {error}", connection.name),
            })
            .collect();
        let text = format!("Connections open to the agent, and their databases:\n{}", lines.join("\n"));
        return Ok((text, format!("{} connections", connections.len())));
    }

    let wanted = needle.map(str::to_lowercase);
    let (mut found, mut failed) = (Vec::new(), Vec::new());
    for connection in connections {
        let databases = match databases_of(connection) {
            Ok(databases) => databases,
            Err(error) => {
                failed.push(format!("- {}: did not answer — {error}", connection.name));
                continue;
            }
        };
        for database in databases {
            match servers.collections(&connection.id, &database) {
                Ok(list) => found.extend(
                    list.into_iter()
                        .filter(|c| wanted.as_ref().is_none_or(|w| c.name.to_lowercase().contains(w)))
                        .map(|c| match c.kind {
                            "collection" => format!("- {} / {database} / {}", connection.name, c.name),
                            kind => format!("- {} / {database} / {} ({kind})", connection.name, c.name),
                        }),
                ),
                Err(error) => failed.push(format!("- {} / {database}: did not answer — {error}", connection.name)),
            }
        }
    }
    let asked = match needle {
        Some(needle) => format!("Collections whose name has \"{needle}\" (connection / database / collection):"),
        None => "Collections (connection / database / collection):".to_string(),
    };
    let mut text = match found.len() {
        0 => format!("{asked}\nnone found."),
        n if n > LISTED => format!("{asked}\n{}\n…and {} more — name a connection or a database, or a longer part of the name.", found[..LISTED].join("\n"), n - LISTED),
        _ => format!("{asked}\n{}", found.join("\n")),
    };
    if !failed.is_empty() {
        text.push_str(&format!("\n\nNot searched:\n{}", failed.join("\n")));
    }
    Ok((text, format!("{} collections", found.len())))
}

fn schema(servers: &dyn MongoServers, target: &Target, at: &str) -> Result<(String, String), ToolError> {
    let sample = Map::from_iter([("$sample".to_string(), json!({ "size": SAMPLE }))]);
    let documents = servers.aggregate(target, vec![sample], SAMPLE)?;
    if documents.is_empty() {
        return Ok((format!("{at} has no documents to sample."), "empty".into()));
    }
    let fields = mongo::schema(&documents);
    let mut lines: Vec<String> = fields
        .iter()
        .take(FIELDS)
        .map(|field| {
            let types: Vec<String> = field.types.iter().map(|(bson, count)| format!("{bson} {count}")).collect();
            format!("- {}: {}", field.path, types.join(", "))
        })
        .collect();
    if fields.len() > FIELDS {
        lines.push(format!("…and {} more fields.", fields.len() - FIELDS));
    }
    let text = format!(
        "Fields of {at}, from {} sampled documents — each BSON type with how many of them have it:\n{}",
        documents.len(),
        lines.join("\n")
    );
    Ok((text, format!("{} fields", fields.len())))
}

fn find(servers: &dyn MongoServers, target: &Target, at: &str, args: &MongoArgs) -> Result<(String, String), ToolError> {
    let limit = args.limit.unwrap_or(FIND_DEFAULT).clamp(1, MOST);
    let spec = FindSpec {
        filter: query(&args.filter, "the filter")?,
        projection: query(&args.projection, "the projection")?,
        sort: query(&args.sort, "the sort")?,
        skip: 0,
        // One past the limit: whether more match.
        limit: limit + 1,
    };
    let documents = servers.find(target, spec)?;
    Ok(shown(documents, limit, &format!("from {at}"), "more match — narrow the filter, or count them"))
}

fn aggregate(servers: &dyn MongoServers, target: &Target, at: &str, args: &MongoArgs) -> Result<(String, String), ToolError> {
    let pipeline = args.pipeline.as_deref().filter(|p| !p.trim().is_empty()).ok_or_else(|| missing(args.op, "pipeline"))?;
    let stages = mongo::parse_pipeline(pipeline)?;
    if let Some(stage) = mongo::writing_stage(&stages) {
        return Err(MongoError::Writes(stage).into());
    }
    let limit = args.limit.unwrap_or(MOST).clamp(1, MOST);
    let results = servers.aggregate(target, stages, limit + 1)?;
    Ok(shown(results, limit, &format!("of the pipeline on {at}"), "more follow — add a $limit or a narrower $match"))
}

/// The first `limit` documents redacted, one to a line, each cut to size;
/// `documents` holds one more when more follow.
fn shown(mut documents: Vec<Value>, limit: usize, what: &str, advice: &str) -> (String, String) {
    let more = documents.len() > limit;
    documents.truncate(limit);
    let lines: Vec<String> = documents
        .iter_mut()
        .map(|document| {
            mongo::redact_document(document);
            cut(&mongo::to_shell_line(document))
        })
        .collect();
    let mut text = format!("{} documents {what}", lines.len());
    if more {
        text.push_str(&format!(" — the first {limit}; {advice}"));
    }
    if !lines.is_empty() {
        text.push_str(":\n");
        text.push_str(&lines.join("\n"));
    }
    (text, format!("{} documents", lines.len()))
}

fn cut(text: &str) -> String {
    let total = text.chars().count();
    match total > DOCUMENT_CHARS {
        true => format!("{}… (cut: {total} characters in all — project fewer fields)", text.chars().take(DOCUMENT_CHARS).collect::<String>()),
        false => text.to_string(),
    }
}

fn indexes(indexes: &[IndexView], at: &str) -> (String, String) {
    let lines: Vec<String> = indexes
        .iter()
        .map(|index| {
            let marks: Vec<String> = [
                index.unique.then(|| "unique".to_string()),
                index.sparse.then(|| "sparse".to_string()),
                index.ttl_seconds.map(|s| format!("TTL {s}s")),
                index.partial.as_ref().map(|p| format!("partial {p}")),
            ]
            .into_iter()
            .flatten()
            .collect();
            match marks.is_empty() {
                true => format!("- {}: {}", index.name, index.keys),
                false => format!("- {}: {} ({})", index.name, index.keys, marks.join(", ")),
            }
        })
        .collect();
    (format!("Indexes of {at}:\n{}", lines.join("\n")), format!("{} indexes", indexes.len()))
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "mongo".to_string(),
        description: "Reads the user's MongoDB databases — only the connections they opened to you on the MongoDB page, by \
            the names they gave them. Nothing is written. Find where a collection is first: op `collections` with `name` \
            (part of a collection's name) searches every open connection and database; without `name` or `database` it lists \
            the connections and their databases. Then on one collection — `connection` (may be left out when only one is \
            open), `database`, `collection`: `schema` shows the fields of a sample with their BSON types — read it before \
            writing a filter, an ObjectId is not a string; `find` returns documents (`filter`, `projection`, `sort`, `limit` up \
            to 50, 20 by default); `count` counts what `filter` matches; `aggregate` runs `pipeline` (up to 50 results, \
            no $out or $merge); `indexes` lists the indexes. Queries are mongosh syntax: `{ status: 'paid', total: { $gt: 100 } }`, \
            `ObjectId('…')`, `ISODate('…')`. Values of fields named like passwords or tokens come back as <redacted>."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["collections", "schema", "find", "count", "aggregate", "indexes"] },
                "connection": { "type": ["string", "null"], "description": "The connection's name, as `collections` lists it. May be left out when only one is open." },
                "database": { "type": ["string", "null"], "description": "For `collections`: only this database. For the rest: the collection's database." },
                "collection": { "type": ["string", "null"], "description": "For every op but `collections`: the collection (or view)." },
                "name": { "type": ["string", "null"], "description": "Only for `collections`: part of a collection's name, any case." },
                "filter": { "type": ["string", "null"], "description": "For `find` and `count`: a query document in mongosh syntax; empty matches all." },
                "projection": { "type": ["string", "null"], "description": "Only for `find`: `{ name: 1, total: 1 }`." },
                "sort": { "type": ["string", "null"], "description": "Only for `find`: `{ createdAt: -1 }`." },
                "pipeline": { "type": ["string", "null"], "description": "Only for `aggregate`: an array of stages in mongosh syntax." },
                "limit": { "type": ["integer", "null"], "description": "For `find` and `aggregate`: at most 50." }
            },
            "required": ["op"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mongo::{CollectionInfo, DatabaseInfo};
    use std::sync::{Arc, Mutex};

    /// Servers in memory: `(connection id, database, collection)` rows, a
    /// connection that does not answer, documents every read returns, and
    /// what was asked of them.
    #[derive(Default)]
    struct Fake {
        connections: Vec<ConnectionView>,
        tree: Vec<(&'static str, &'static str, &'static str)>,
        down: Vec<&'static str>,
        documents: Vec<Value>,
        asked: Mutex<Vec<String>>,
    }

    impl Fake {
        fn ask(&self, what: String) {
            self.asked.lock().unwrap().push(what);
        }
        fn answer(&self, connection: &str) -> Result<(), MongoError> {
            match self.down.contains(&connection) {
                true => Err(MongoError::Server("connection refused".into())),
                false => Ok(()),
            }
        }
    }

    impl MongoServers for Fake {
        fn connections(&self) -> Result<Vec<ConnectionView>, MongoError> {
            Ok(self.connections.clone())
        }
        fn databases(&self, connection: &str) -> Result<Vec<DatabaseInfo>, MongoError> {
            self.ask(format!("databases {connection}"));
            self.answer(connection)?;
            let mut names: Vec<&str> = self.tree.iter().filter(|(c, ..)| *c == connection).map(|(_, d, _)| *d).collect();
            names.dedup();
            Ok(names.into_iter().map(|name| DatabaseInfo { name: name.into(), size_on_disk: 0 }).collect())
        }
        fn collections(&self, connection: &str, database: &str) -> Result<Vec<CollectionInfo>, MongoError> {
            self.ask(format!("collections {connection} {database}"));
            self.answer(connection)?;
            Ok(self
                .tree
                .iter()
                .filter(|(c, d, _)| *c == connection && *d == database)
                .map(|(_, _, name)| CollectionInfo { name: name.to_string(), kind: if name.ends_with("_view") { "view" } else { "collection" } })
                .collect())
        }
        fn find(&self, target: &Target, spec: FindSpec) -> Result<Vec<Value>, MongoError> {
            self.ask(format!("find {} {}.{} limit {} filter {}", target.connection, target.database, target.collection, spec.limit, Value::Object(spec.filter)));
            Ok(self.documents.iter().take(spec.limit).cloned().collect())
        }
        fn count(&self, target: &Target, filter: Map<String, Value>) -> Result<u64, MongoError> {
            self.ask(format!("count {} {}", target.collection, Value::Object(filter)));
            Ok(7)
        }
        fn aggregate(&self, target: &Target, stages: Vec<Map<String, Value>>, cap: usize) -> Result<Vec<Value>, MongoError> {
            self.ask(format!("aggregate {} cap {cap} {}", target.collection, Value::Array(stages.into_iter().map(Value::Object).collect())));
            Ok(self.documents.iter().take(cap).cloned().collect())
        }
        fn indexes(&self, target: &Target) -> Result<Vec<IndexView>, MongoError> {
            self.ask(format!("indexes {}", target.collection));
            Ok(vec![
                IndexView { name: "_id_".into(), keys: "{ _id: 1 }".into(), unique: false, sparse: false, ttl_seconds: None, partial: None },
                IndexView { name: "email_1".into(), keys: "{ email: 1 }".into(), unique: true, sparse: false, ttl_seconds: Some(60), partial: None },
            ])
        }
    }

    fn view(id: &str, name: &str, agent: bool) -> ConnectionView {
        ConnectionView { id: id.into(), name: name.into(), address: format!("mongodb://{id}"), agent }
    }

    /// Local and Staging open to the agent, Prod not; each with a `shop`.
    fn servers() -> Fake {
        Fake {
            connections: vec![view("c1", "Local", true), view("c2", "Prod", false), view("c3", "Staging", true)],
            tree: vec![
                ("c1", "shop", "orders"),
                ("c1", "shop", "users"),
                ("c1", "admin", "system.orders"),
                ("c1", "billing", "Order_view"),
                ("c2", "shop", "orders"),
                ("c3", "shop", "carts"),
            ],
            ..Fake::default()
        }
    }

    fn run(fake: Fake, args: Value) -> (Result<ToolResult, ToolError>, Vec<String>) {
        let fake = Arc::new(fake);
        let deps = ToolDeps { mongo: Some(fake.clone()), ..ToolDeps::default() };
        let result = mongo(&serde_json::from_value(args).unwrap(), &deps);
        let asked = fake.asked.lock().unwrap().clone();
        (result, asked)
    }

    fn said(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(ToolResult::Mongo { text, .. }) => text,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_collection_is_found_by_part_of_its_name_in_every_open_connection_and_database() {
        let (result, asked) = run(servers(), json!({ "op": "collections", "name": "ORDER" }));
        let text = said(result);
        assert!(text.contains("- Local / shop / orders\n- Local / billing / Order_view (view)"), "{text}");
        assert!(!text.contains("users") && !text.contains("carts"), "only what the name matches: {text}");
        assert!(!text.contains("system.orders"), "the server's own databases unless named: {text}");
        assert!(asked.iter().all(|a| !a.contains("c2")), "a connection not opened is not asked: {asked:?}");

        // Named, a system database is searched.
        let text = said(run(servers(), json!({ "op": "collections", "database": "admin", "connection": "local" })).0);
        assert!(text.contains("- Local / admin / system.orders"), "{text}");
    }

    #[test]
    fn a_server_that_does_not_answer_is_said_and_the_others_still_searched() {
        let fake = || Fake { down: vec!["c1"], ..servers() };
        let text = said(run(fake(), json!({ "op": "collections", "name": "cart" })).0);
        assert!(text.contains("- Staging / shop / carts"), "{text}");
        assert!(text.contains("Not searched:\n- Local: did not answer — connection refused"), "{text}");
        // A database named is asked for its collections straight away.
        let text = said(run(fake(), json!({ "op": "collections", "database": "shop" })).0);
        assert!(text.contains("- Staging / shop / carts\n\nNot searched:\n- Local / shop: did not answer — connection refused"), "{text}");
    }

    #[test]
    fn without_a_name_the_open_connections_list_their_databases() {
        let fake = Fake { down: vec!["c3"], ..servers() };
        let text = said(run(fake, json!({ "op": "collections" })).0);
        assert_eq!(text, "Connections open to the agent, and their databases:\n- Local: shop, billing\n- Staging: did not answer — connection refused");
    }

    #[test]
    fn a_long_list_is_cut_and_counted() {
        let names: Vec<&'static str> = (0..LISTED + 3).map(|n| &*Box::leak(format!("orders{n}").into_boxed_str())).collect();
        let fake = Fake { tree: names.into_iter().map(|n| ("c1", "shop", n)).collect(), ..servers() };
        let text = said(run(fake, json!({ "op": "collections", "name": "orders" })).0);
        assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), LISTED);
        assert!(text.contains("…and 3 more"), "{text}");
    }

    #[test]
    fn only_a_connection_opened_to_the_agent_is_reached_by_its_name() {
        let (result, asked) = run(servers(), json!({ "op": "find", "connection": "Prod", "database": "shop", "collection": "orders" }));
        let error = result.unwrap_err().to_string();
        assert!(error.contains("no connection named \"Prod\" is open") && error.contains("\"Local\", \"Staging\""), "{error}");
        assert!(asked.is_empty(), "{asked:?}");

        let closed = Fake { connections: vec![view("c2", "Prod", false)], ..servers() };
        assert!(matches!(run(closed, json!({ "op": "collections" })).0, Err(ToolError::Mongo(MongoError::NoneOpen))));
        let off = mongo(&serde_json::from_value(json!({ "op": "collections" })).unwrap(), &ToolDeps::default());
        assert!(matches!(off, Err(ToolError::Mongo(MongoError::NoneOpen))));
    }

    #[test]
    fn the_connection_may_be_left_out_only_when_one_is_open() {
        // An empty or "null" name is no name: a search needs none.
        for none in ["", "null"] {
            let text = said(run(servers(), json!({ "op": "collections", "name": "cart", "connection": none })).0);
            assert!(text.contains("- Staging / shop / carts"), "{none:?}: {text}");
        }
        let error = run(servers(), json!({ "op": "count", "database": "shop", "collection": "orders" })).0.unwrap_err().to_string();
        assert!(error.contains("needs `connection`, one of \"Local\", \"Staging\""), "{error}");

        let one = Fake { connections: vec![view("c1", "Local", true), view("c2", "Prod", false)], ..servers() };
        let (result, asked) = run(one, json!({ "op": "count", "database": "shop", "collection": "orders", "filter": { "paid": true } }));
        assert_eq!(said(result), "7 documents in shop.orders on Local match.");
        assert_eq!(asked, ["count orders {\"paid\":true}"], "an object sent for the filter is read as its JSON");

        let twins = Fake { connections: vec![view("c1", "Local", true), view("c3", "local", true)], ..servers() };
        let error = run(twins, json!({ "op": "count", "connection": "Local", "database": "shop", "collection": "orders" })).0.unwrap_err().to_string();
        assert!(error.contains("2 connections open to the agent are named \"Local\""), "{error}");
    }

    #[test]
    fn an_op_on_a_collection_needs_its_database_and_collection() {
        for args in [json!({ "op": "find", "connection": "Local", "collection": "orders" }), json!({ "op": "indexes", "connection": "Local", "database": "shop", "collection": " " })] {
            let (result, asked) = run(servers(), args);
            assert!(matches!(result, Err(ToolError::InvalidArguments { .. })), "{result:?}");
            assert!(asked.is_empty());
        }
        let error = run(servers(), json!({ "op": "aggregate", "connection": "Local", "database": "shop", "collection": "orders" })).0.unwrap_err().to_string();
        assert!(error.contains("op \"aggregate\" needs `pipeline`"), "{error}");
    }

    #[test]
    fn a_find_is_limited_says_when_more_match_and_is_read_before_the_server_is_asked() {
        let documents: Vec<Value> = (0..60).map(|n| json!({ "_id": { "$numberInt": n.to_string() } })).collect();
        let fake = || Fake { documents: documents.clone(), ..servers() };
        let at = json!({ "op": "find", "connection": "Local", "database": "shop", "collection": "orders" });
        let with = |extra: Value| {
            let mut args = at.clone();
            args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            args
        };

        let (result, asked) = run(fake(), with(json!({ "filter": "{ _id: { $lt: 5 } }" })));
        assert_eq!(asked, ["find c1 shop.orders limit 21 filter {\"_id\":{\"$lt\":5}}"]);
        let text = said(result);
        assert!(text.starts_with("20 documents from shop.orders on Local — the first 20; more match"), "{text}");
        assert_eq!(text.lines().count(), 21);

        let (result, asked) = run(fake(), with(json!({ "limit": 500 })));
        assert!(asked[0].contains("limit 51"), "{asked:?}");
        assert!(said(result).starts_with("50 documents"));

        let few = Fake { documents: documents[..3].to_vec(), ..servers() };
        assert!(said(run(few, at.clone()).0).starts_with("3 documents from shop.orders on Local:\n{ _id: 0 }"));

        let (result, asked) = run(fake(), with(json!({ "filter": "{ _id: " })));
        assert!(matches!(result, Err(ToolError::Mongo(MongoError::BadInput { what: "the filter", .. }))), "{result:?}");
        assert!(asked.is_empty());
    }

    #[test]
    fn what_the_model_reads_has_its_secrets_redacted_and_is_cut_to_size() {
        let long = "x".repeat(DOCUMENT_CHARS + 10);
        let fake = Fake {
            documents: vec![json!({
                "user": "ann",
                "passwordHash": "hunter2",
                "api_key": { "v": "k1" },
                "dsn": "mongodb://ann:hunter2@db/x",
                "profile": [{ "refresh_token": "t0k" }],
                "clientSecret": "s3",
                "old_passwd": "p4",
                "credentials": { "SCRAM-SHA-256": { "storedKey": "c5" } },
                "privateKey": "k6",
                "note": long
            })],
            ..servers()
        };
        let text = said(run(fake, json!({ "op": "find", "connection": "Local", "database": "shop", "collection": "users" })).0);
        for secret in ["hunter2", "k1", "t0k", "s3", "p4", "c5", "k6"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
        assert!(text.contains("passwordHash: '<redacted>'") || text.contains("passwordHash: \"<redacted>\""), "{text}");
        assert!(text.contains("user: "), "{text}");
        let (kept, rest) = text.lines().nth(1).unwrap().split_once("… (cut: ").unwrap();
        assert_eq!(kept.chars().count(), DOCUMENT_CHARS);
        assert!(rest.ends_with(" characters in all — project fewer fields)"), "{rest}");
    }

    #[test]
    fn a_pipeline_that_writes_is_refused_before_the_server_is_asked() {
        for pipeline in ["[{ $match: {} }, { $out: 'copy' }]", "[{ $merge: { into: 'copy' } }]"] {
            let (result, asked) = run(servers(), json!({ "op": "aggregate", "connection": "Local", "database": "shop", "collection": "orders", "pipeline": pipeline }));
            assert!(matches!(result, Err(ToolError::Mongo(MongoError::Writes(_)))), "{result:?}");
            assert!(asked.is_empty(), "{asked:?}");
        }
        let fake = Fake { documents: (0..60).map(|n| json!({ "n": n })).collect(), ..servers() };
        let (result, asked) = run(fake, json!({ "op": "aggregate", "connection": "Local", "database": "shop", "collection": "orders", "pipeline": "[{ $match: { n: { $gte: 0 } } }]", "limit": 5 }));
        assert_eq!(asked, ["aggregate orders cap 6 [{\"$match\":{\"n\":{\"$gte\":0}}}]"]);
        assert!(said(result).starts_with("5 documents of the pipeline on shop.orders on Local — the first 5; more follow"));
        let fake = Fake { documents: (0..60).map(|n| json!({ "n": n })).collect(), ..servers() };
        let (result, asked) = run(fake, json!({ "op": "aggregate", "connection": "Local", "database": "shop", "collection": "orders", "pipeline": "[]", "limit": 500 }));
        assert!(asked[0].starts_with("aggregate orders cap 51 "), "{asked:?}");
        assert!(said(result).starts_with("50 documents"));
    }

    #[test]
    fn a_schema_is_the_fields_of_a_sample_with_their_types() {
        let fake = Fake {
            documents: vec![
                json!({ "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" }, "total": { "$numberInt": "5" }, "items": [{ "sku": "a" }, { "sku": "b" }] }),
                json!({ "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d1" }, "total": null }),
                json!({ "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d2" }, "total": null }),
            ],
            ..servers()
        };
        let (result, asked) = run(fake, json!({ "op": "schema", "connection": "Local", "database": "shop", "collection": "orders" }));
        assert_eq!(asked, ["aggregate orders cap 100 [{\"$sample\":{\"size\":100}}]"]);
        assert_eq!(
            said(result),
            "Fields of shop.orders on Local, from 3 sampled documents — each BSON type with how many of them have it:\n\
             - _id: ObjectId 3\n- total: Null 2, Int32 1\n- items: Array 1\n- items.sku: String 1"
        );
        let empty = run(servers(), json!({ "op": "schema", "connection": "Local", "database": "shop", "collection": "orders" })).0;
        assert_eq!(said(empty), "shop.orders on Local has no documents to sample.");

        let wide = Fake { documents: vec![Value::Object((0..FIELDS + 2).map(|n| (format!("f{n}"), json!(n))).collect())], ..servers() };
        let text = said(run(wide, json!({ "op": "schema", "connection": "Local", "database": "shop", "collection": "orders" })).0);
        assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), FIELDS);
        assert!(text.ends_with("…and 2 more fields."), "{text}");
    }

    #[test]
    fn indexes_are_listed_with_what_marks_them() {
        let text = said(run(servers(), json!({ "op": "indexes", "connection": "Local", "database": "shop", "collection": "users" })).0);
        assert_eq!(text, "Indexes of shop.users on Local:\n- _id_: { _id: 1 }\n- email_1: { email: 1 } (unique, TTL 60s)");
    }
}
