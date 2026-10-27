//! The MongoDB page's one way to a server (`docs/43-mongodb.md`): the
//! official driver's blocking API, every call already off the main thread
//! (`commands::blocking`). The only module that opens a connection to one —
//! `data_policy::NETWORK_ALLOWED` names it, and the driver's paths anywhere
//! else fail that test.
//!
//! In and out goes Extended JSON (`domain::mongo` reads and writes it as
//! mongosh text): documents leave as canonical Extended JSON, so every BSON
//! type keeps its name on the way to the window and back.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use mongodb::bson::{self, Bson, Document};
use mongodb::options::{ClientOptions, IndexOptions};
use mongodb::sync::{Client, Collection};
use mongodb::IndexModel;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::domain::mongo::{to_shell_line, IndexView, MongoError};

/// How long a server may take to be found: a wrong host says so in seconds,
/// not in the driver's thirty.
const SELECT_TIMEOUT: Duration = Duration::from_secs(6);
/// What one query may take on the server before it is stopped there.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// The clients made, one per URI and kept: a client is a pool, and making one
/// is a handshake (with TLS and SCRAM, several round trips) on every call.
/// Keyed by the URI's hash, so an edited connection gets a new one.
#[derive(Default)]
pub struct MongoClients {
    clients: Mutex<HashMap<String, Client>>,
}

impl MongoClients {
    fn client(&self, uri: &SecretString) -> Result<Client, MongoError> {
        let key = blake3::hash(uri.expose_secret().as_bytes()).to_hex().to_string();
        if let Some(client) = self.clients.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(client.clone());
        }
        let mut options = ClientOptions::parse(uri.expose_secret()).run().map_err(|e| MongoError::BadUri(e.to_string()))?;
        options.app_name = Some("Kibo Agent".into());
        options.server_selection_timeout = Some(SELECT_TIMEOUT);
        options.connect_timeout = Some(SELECT_TIMEOUT);
        let client = Client::with_options(options).map_err(server)?;
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).insert(key, client.clone());
        Ok(client)
    }

    fn collection(&self, uri: &SecretString, db: &str, name: &str) -> Result<Collection<Document>, MongoError> {
        Ok(self.client(uri)?.database(db).collection(name))
    }

    /// The server's version: that the URI reaches it and the login holds.
    pub fn ping(&self, uri: &SecretString) -> Result<String, MongoError> {
        let client = self.client(uri)?;
        let info = client.database("admin").run_command(bson::doc! { "buildInfo": 1 }).run().map_err(server)?;
        Ok(info.get_str("version").unwrap_or("unknown").to_string())
    }

    pub fn databases(&self, uri: &SecretString) -> Result<Vec<DatabaseInfo>, MongoError> {
        let client = self.client(uri)?;
        let specs = client.list_databases().run().map_err(server)?;
        Ok(specs.into_iter().map(|spec| DatabaseInfo { name: spec.name, size_on_disk: spec.size_on_disk }).collect())
    }

    pub fn collections(&self, uri: &SecretString, db: &str) -> Result<Vec<CollectionInfo>, MongoError> {
        let client = self.client(uri)?;
        let mut found: Vec<CollectionInfo> = client
            .database(db)
            .list_collections()
            .run()
            .map_err(server)?
            .map(|spec| {
                spec.map(|spec| CollectionInfo {
                    kind: match spec.collection_type {
                        mongodb::results::CollectionType::View => "view",
                        mongodb::results::CollectionType::Timeseries => "timeseries",
                        _ => "collection",
                    },
                    name: spec.name,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(server)?;
        found.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(found)
    }

    pub fn find(&self, uri: &SecretString, db: &str, collection: &str, spec: FindSpec) -> Result<Vec<Value>, MongoError> {
        let collection = self.collection(uri, db, collection)?;
        let mut find = collection.find(document(spec.filter)?).skip(spec.skip).limit(spec.limit).max_time(QUERY_TIMEOUT);
        if !spec.projection.is_empty() {
            find = find.projection(document(spec.projection)?);
        }
        if !spec.sort.is_empty() {
            find = find.sort(document(spec.sort)?);
        }
        find.run().map_err(server)?.map(|doc| doc.map(canonical).map_err(server)).collect()
    }

    /// Every match counted, or — with no filter — the collection's own count
    /// from its metadata, which does not read it.
    pub fn count(&self, uri: &SecretString, db: &str, collection: &str, filter: Map<String, Value>) -> Result<u64, MongoError> {
        let collection = self.collection(uri, db, collection)?;
        if filter.is_empty() {
            return collection.estimated_document_count().max_time(QUERY_TIMEOUT).run().map_err(server);
        }
        collection.count_documents(document(filter)?).max_time(QUERY_TIMEOUT).run().map_err(server)
    }

    /// The first `cap` results: a pipeline can return the whole collection.
    pub fn aggregate(&self, uri: &SecretString, db: &str, collection: &str, pipeline: Vec<Map<String, Value>>, cap: usize) -> Result<Vec<Value>, MongoError> {
        let collection = self.collection(uri, db, collection)?;
        let stages = pipeline.into_iter().map(document).collect::<Result<Vec<_>, _>>()?;
        collection.aggregate(stages).max_time(QUERY_TIMEOUT).run().map_err(server)?.take(cap).map(|doc| doc.map(canonical).map_err(server)).collect()
    }

    /// How many went in.
    pub fn insert(&self, uri: &SecretString, db: &str, collection: &str, documents: Vec<Map<String, Value>>) -> Result<usize, MongoError> {
        let collection = self.collection(uri, db, collection)?;
        let documents = documents.into_iter().map(document).collect::<Result<Vec<_>, _>>()?;
        Ok(collection.insert_many(documents).run().map_err(server)?.inserted_ids.len())
    }

    /// The document with `id` replaced by `replacement`.
    pub fn replace(&self, uri: &SecretString, db: &str, collection: &str, id: Value, replacement: Map<String, Value>) -> Result<(), MongoError> {
        let collection = self.collection(uri, db, collection)?;
        let id = bson_of(id)?;
        let replacement = document(replacement)?;
        // Without one it keeps its own; with another, `replace_one` would fail
        // on the server with a message about an immutable field.
        if replacement.get("_id").is_some_and(|given| *given != id) {
            return Err(MongoError::IdChanged);
        }
        let replaced = collection.replace_one(bson::doc! { "_id": id }, replacement).run().map_err(server)?;
        if replaced.matched_count == 0 {
            return Err(MongoError::Gone);
        }
        Ok(())
    }

    pub fn delete(&self, uri: &SecretString, db: &str, collection: &str, id: Value) -> Result<(), MongoError> {
        let collection = self.collection(uri, db, collection)?;
        let deleted = collection.delete_one(bson::doc! { "_id": bson_of(id)? }).run().map_err(server)?;
        if deleted.deleted_count == 0 {
            return Err(MongoError::Gone);
        }
        Ok(())
    }

    pub fn indexes(&self, uri: &SecretString, db: &str, collection: &str) -> Result<Vec<IndexView>, MongoError> {
        let collection = self.collection(uri, db, collection)?;
        collection
            .list_indexes()
            .run()
            .map_err(server)?
            .map(|index| {
                let index = index.map_err(server)?;
                let options = index.options.unwrap_or_default();
                Ok(IndexView {
                    name: options.name.unwrap_or_default(),
                    keys: to_shell_line(&canonical(index.keys)),
                    unique: options.unique.unwrap_or(false),
                    sparse: options.sparse.unwrap_or(false),
                    ttl_seconds: options.expire_after.map(|d| d.as_secs()),
                    partial: options.partial_filter_expression.map(|filter| to_shell_line(&canonical(filter))),
                })
            })
            .collect()
    }

    /// The index's name, the server's if none was given.
    pub fn create_index(&self, uri: &SecretString, db: &str, collection: &str, keys: Map<String, Value>, unique: bool, name: Option<String>) -> Result<String, MongoError> {
        let collection = self.collection(uri, db, collection)?;
        let options = IndexOptions::builder().unique(unique.then_some(true)).name(name).build();
        let model = IndexModel::builder().keys(document(keys)?).options(options).build();
        Ok(collection.create_index(model).run().map_err(server)?.index_name)
    }

    pub fn drop_index(&self, uri: &SecretString, db: &str, collection: &str, name: &str) -> Result<(), MongoError> {
        let collection = self.collection(uri, db, collection)?;
        collection.drop_index(name).run().map_err(server)
    }

    /// In a database not there yet, it makes the database too.
    pub fn create_collection(&self, uri: &SecretString, db: &str, collection: &str) -> Result<(), MongoError> {
        self.client(uri)?.database(db).create_collection(collection).run().map_err(server)
    }

    pub fn drop_collection(&self, uri: &SecretString, db: &str, collection: &str) -> Result<(), MongoError> {
        self.collection(uri, db, collection)?.drop().run().map_err(server)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    pub size_on_disk: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionInfo {
    pub name: String,
    /// `collection`, `view` or `timeseries`.
    pub kind: &'static str,
}

/// A find, its parts already read from the page's text.
#[derive(Debug, Clone, Default)]
pub struct FindSpec {
    pub filter: Map<String, Value>,
    pub projection: Map<String, Value>,
    pub sort: Map<String, Value>,
    pub skip: u64,
    pub limit: i64,
}

fn server(error: mongodb::error::Error) -> MongoError {
    MongoError::Server(error.to_string())
}

fn bson_of(value: Value) -> Result<Bson, MongoError> {
    Bson::try_from(value).map_err(|e| MongoError::Server(format!("not a value MongoDB can store: {e}")))
}

fn document(map: Map<String, Value>) -> Result<Document, MongoError> {
    Document::try_from(map).map_err(|e| MongoError::Server(format!("not a document MongoDB can store: {e}")))
}

fn canonical(doc: Document) -> Value {
    Bson::Document(doc).into_canonical_extjson()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mongo::{parse, parse_document, to_shell};
    use serde_json::json;

    /// The domain's text and the driver agree on every type: what `parse`
    /// makes is BSON of the type it names, and that BSON written out is the text again.
    #[test]
    fn the_text_and_bson_agree_on_types() {
        let text = r#"{
  _id: ObjectId("65f0a1b2c3d4e5f6a7b8c9d0"),
  n32: 5,
  n64: NumberLong(5),
  dbl: 5.0,
  dec: NumberDecimal("1.10"),
  at: ISODate("2024-05-01T10:00:00.123Z"),
  id: UUID("01234567-89ab-cdef-0123-456789abcdef"),
  re: /a\/b/i,
  ts: Timestamp({ t: 1, i: 2 }),
  list: [
    1,
    "two",
    null
  ]
}"#;
        let doc = document(parse_document(text, "the document").unwrap()).unwrap();
        assert!(matches!(doc.get("n32"), Some(Bson::Int32(5))));
        assert!(matches!(doc.get("n64"), Some(Bson::Int64(5))));
        assert!(matches!(doc.get("dbl"), Some(Bson::Double(d)) if *d == 5.0));
        assert!(matches!(doc.get("_id"), Some(Bson::ObjectId(_))));
        assert!(matches!(doc.get("at"), Some(Bson::DateTime(_))));
        assert!(matches!(doc.get("id"), Some(Bson::Binary(b)) if b.subtype == bson::spec::BinarySubtype::Uuid));
        assert_eq!(to_shell(&canonical(doc)), text);
        // A relaxed `$date` and an `_id` sent back by the window read too.
        assert!(matches!(bson_of(parse("ISODate('2024-05-01')").unwrap()).unwrap(), Bson::DateTime(_)));
        assert!(matches!(bson_of(json!({ "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" })).unwrap(), Bson::ObjectId(_)));
    }

    #[test]
    fn a_wrapper_with_a_bad_value_is_refused_not_stored_as_text() {
        assert!(document(parse_document("{ d: NumberDecimal('abc') }", "the document").unwrap()).is_err());
    }

    /// A real server, at `KIBO_MONGO_URI` (default `mongodb://localhost:27017`):
    /// a scratch collection made, written, read, indexed and dropped.
    #[test]
    #[ignore = "needs a MongoDB server — set KIBO_MONGO_URI"]
    fn live_server() {
        let uri = SecretString::from(std::env::var("KIBO_MONGO_URI").unwrap_or_else(|_| "mongodb://localhost:27017".into()));
        let clients = MongoClients::default();
        let db = "kibo_live_test";
        let coll = format!("c{}", std::process::id());
        let version = clients.ping(&uri).unwrap();
        assert!(!version.is_empty());

        clients.create_collection(&uri, db, &coll).unwrap();
        assert!(clients.collections(&uri, db).unwrap().iter().any(|c| c.name == coll && c.kind == "collection"));
        assert!(clients.databases(&uri).unwrap().iter().any(|d| d.name == db));

        let docs = crate::domain::mongo::parse_documents("[{ _id: 1, n: NumberLong(5), name: 'a' }, { _id: 2, name: 'b' }, { _id: 3, name: 'c' }]", "the documents").unwrap();
        assert_eq!(clients.insert(&uri, db, &coll, docs).unwrap(), 3);
        assert_eq!(clients.count(&uri, db, &coll, Map::new()).unwrap(), 3);
        assert_eq!(clients.count(&uri, db, &coll, parse_document("{ name: /^[ab]$/ }", "f").unwrap()).unwrap(), 2);

        let spec = FindSpec { sort: parse_document("{ _id: -1 }", "s").unwrap(), projection: parse_document("{ name: 1 }", "p").unwrap(), limit: 2, skip: 0, ..Default::default() };
        let found = clients.find(&uri, db, &coll, spec).unwrap();
        assert_eq!(found, vec![json!({ "_id": { "$numberInt": "3" }, "name": "c" }), json!({ "_id": { "$numberInt": "2" }, "name": "b" })]);

        let first = clients.find(&uri, db, &coll, FindSpec { filter: parse_document("{ _id: 1 }", "f").unwrap(), limit: 1, ..Default::default() }).unwrap();
        assert_eq!(first[0]["n"], json!({ "$numberLong": "5" }), "an Int64 comes back as one");

        clients.replace(&uri, db, &coll, json!(1), parse_document("{ _id: 1, name: 'z' }", "d").unwrap()).unwrap();
        assert!(matches!(clients.replace(&uri, db, &coll, json!(1), parse_document("{ _id: 9 }", "d").unwrap()), Err(MongoError::IdChanged)));
        let grouped = clients.aggregate(&uri, db, &coll, crate::domain::mongo::parse_pipeline("[{ $match: { name: 'z' } }, { $project: { _id: 0, name: 1 } }]").unwrap(), 10).unwrap();
        assert_eq!(grouped, vec![json!({ "name": "z" })]);
        assert_eq!(clients.aggregate(&uri, db, &coll, Vec::new(), 2).unwrap().len(), 2, "capped");

        let name = clients.create_index(&uri, db, &coll, parse_document("{ name: 1 }", "k").unwrap(), true, None).unwrap();
        let indexes = clients.indexes(&uri, db, &coll).unwrap();
        let made = indexes.iter().find(|i| i.name == name).unwrap();
        assert!(made.unique);
        assert_eq!(made.keys, "{ name: 1 }");
        clients.drop_index(&uri, db, &coll, &name).unwrap();

        clients.delete(&uri, db, &coll, json!(2)).unwrap();
        assert!(matches!(clients.delete(&uri, db, &coll, json!(2)), Err(MongoError::Gone)));
        assert!(matches!(clients.replace(&uri, db, &coll, json!(2), Map::new()), Err(MongoError::Gone)));

        clients.drop_collection(&uri, db, &coll).unwrap();
        assert!(!clients.collections(&uri, db).unwrap().iter().any(|c| c.name == coll));
    }

    #[test]
    #[ignore = "waits for the server selection timeout"]
    fn an_unreachable_server_says_so_in_seconds() {
        let started = std::time::Instant::now();
        let error = MongoClients::default().ping(&SecretString::from("mongodb://127.0.0.1:1")).unwrap_err();
        assert!(started.elapsed() < SELECT_TIMEOUT * 2, "{:?}", started.elapsed());
        assert!(matches!(error, MongoError::Server(_)), "{error}");
    }
}
