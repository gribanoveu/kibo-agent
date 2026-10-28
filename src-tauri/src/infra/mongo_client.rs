//! The MongoDB page's one way to a server (`docs/44-mongodb.md`): the
//! official driver's blocking API, every call already off the main thread
//! (`commands::blocking`). The only module that opens a connection to one —
//! `data_policy::NETWORK_ALLOWED` names it, and the driver's paths anywhere
//! else fail that test.
//!
//! In and out goes Extended JSON (`domain::mongo` reads and writes it as
//! mongosh text): documents leave as canonical Extended JSON, so every BSON
//! type keeps its name on the way to the window and back.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use mongodb::bson::{self, Bson, Document};
use mongodb::options::{ClientOptions, ConnectionString, IndexOptions};
pub use mongodb::sync::Client;
use mongodb::sync::{Collection, Cursor};
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
/// How many finds stay open between pages; past it, the oldest is closed.
const MAX_OPEN_FINDS: usize = 32;

/// The clients made, one per saved connection and kept: a client is a pool,
/// and making one is a handshake (with TLS and SCRAM, several round trips) on
/// every call. Kept by the connection's id, so a call does not open the sealed
/// store to find its URI; a connection changed or deleted is [`forget`]-ten,
/// and its pool and monitors go with it.
///
/// A find stays open between pages, so the next page goes on where the last
/// stopped instead of skipping again from the start — on a view, running its
/// pipeline again.
///
/// [`forget`]: MongoClients::forget
#[derive(Default)]
pub struct MongoClients {
    clients: Mutex<HashMap<String, Client>>,
    finds: Mutex<OpenFinds>,
}

/// The finds open, by the number a page reads its next from: a new one per
/// page, so a page asked for twice is read once.
#[derive(Default)]
struct OpenFinds {
    next: u64,
    open: BTreeMap<u64, OpenFind>,
}

/// A find between pages: its cursor, the document read past the last page —
/// how a page knows another follows — and what opens it again where it was
/// when the server has let the cursor go, idle for ten minutes.
struct OpenFind {
    cursor: Cursor<Document>,
    ahead: Option<Value>,
    from: FindFrom,
    /// Read from the find so far, `ahead` among them.
    read: u64,
}

/// Where a find runs: the client, and the connection it was made for.
pub struct Place<'a> {
    pub connection: &'a str,
    pub client: &'a Client,
}

struct FindFrom {
    connection: String,
    client: Client,
    db: String,
    collection: String,
    spec: FindSpec,
}

/// A page of a find, and where its next page is read from — `None` after the last.
#[derive(Debug)]
pub struct Found {
    pub documents: Vec<Value>,
    pub next: Option<u64>,
}

/// `uri` as the driver reads it, with nothing connected or looked up.
pub fn check(uri: &SecretString) -> Result<(), MongoError> {
    ConnectionString::parse(uri.expose_secret()).map(drop).map_err(unread)
}

/// A client for `uri`, not connected yet: the first call connects. Kept by
/// nobody — a connection only tested goes when its test is done.
pub fn open(uri: &SecretString) -> Result<Client, MongoError> {
    let mut options = ClientOptions::parse(uri.expose_secret()).run().map_err(unread)?;
    options.app_name = Some("Kibo Agent".into());
    options.server_selection_timeout = Some(SELECT_TIMEOUT);
    options.connect_timeout = Some(SELECT_TIMEOUT);
    Client::with_options(options).map_err(server)
}

impl MongoClients {
    /// The saved connection's client, made from `uri` the first time.
    ///
    /// The lock is held while it is made, so a connection changed meanwhile
    /// is forgotten after the old client went in, never before.
    // ponytail: one lock for every connection — a slow `mongodb+srv://` lookup holds up the others' first call; per-connection slots if that shows.
    pub fn client(&self, connection: &str, uri: impl FnOnce() -> Result<SecretString, MongoError>) -> Result<Client, MongoError> {
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(client) = clients.get(connection) {
            return Ok(client.clone());
        }
        let client = open(&uri()?)?;
        clients.insert(connection.to_string(), client.clone());
        Ok(client)
    }

    /// A connection changed or deleted: its client and its finds let go, made
    /// again from the saved URI on the next call.
    pub fn forget(&self, connection: &str) {
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(connection);
        self.finds.lock().unwrap_or_else(|e| e.into_inner()).open.retain(|_, find| find.from.connection != connection);
    }

    /// Every connection deleted at once — the feature's data.
    pub fn forget_all(&self) {
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.finds.lock().unwrap_or_else(|e| e.into_inner()).open.clear();
    }

    /// The first page of a find, its size `spec.limit`.
    pub fn find(&self, place: Place, db: &str, collection: &str, spec: FindSpec) -> Result<Found, MongoError> {
        let from = FindFrom { connection: place.connection.into(), client: place.client.clone(), db: db.into(), collection: collection.into(), spec };
        let cursor = cursor(&from, 0)?;
        self.page(OpenFind { cursor, ahead: None, from, read: 0 })
    }

    /// The page after the one `next` came with.
    pub fn find_more(&self, next: u64) -> Result<Found, MongoError> {
        let open = self.finds.lock().unwrap_or_else(|e| e.into_inner()).open.remove(&next).ok_or(MongoError::FindClosed)?;
        self.page(open)
    }

    /// Dropped, the cursor is closed on the server too.
    pub fn find_close(&self, next: u64) {
        self.finds.lock().unwrap_or_else(|e| e.into_inner()).open.remove(&next);
    }

    /// The next `limit` documents, and one read past them: kept open when there is one.
    fn page(&self, mut open: OpenFind) -> Result<Found, MongoError> {
        let limit = open.from.spec.limit;
        let mut documents: Vec<Value> = open.ahead.take().into_iter().collect();
        while documents.len() <= limit {
            let next = match open.cursor.next() {
                Some(Err(error)) if cursor_gone(&error) => {
                    open.cursor = cursor(&open.from, open.read)?;
                    open.cursor.next()
                }
                next => next,
            };
            let Some(doc) = next else { break };
            documents.push(canonical(doc.map_err(server)?));
            open.read += 1;
        }
        if documents.len() <= limit {
            return Ok(Found { documents, next: None });
        }
        open.ahead = documents.pop();
        let mut finds = self.finds.lock().unwrap_or_else(|e| e.into_inner());
        let next = finds.next;
        finds.next += 1;
        finds.open.insert(next, open);
        while finds.open.len() > MAX_OPEN_FINDS {
            finds.open.pop_first();
        }
        Ok(Found { documents, next: Some(next) })
    }
}

/// `from`'s find, past what was read of it.
fn cursor(from: &FindFrom, read: u64) -> Result<Cursor<Document>, MongoError> {
    let spec = &from.spec;
    let batch = u32::try_from(spec.limit + 1).unwrap_or(u32::MAX);
    let collection = from.client.database(&from.db).collection::<Document>(&from.collection);
    let mut find = collection.find(document(spec.filter.clone())?).skip(spec.skip + read).batch_size(batch).max_time(QUERY_TIMEOUT);
    if !spec.projection.is_empty() {
        find = find.projection(document(spec.projection.clone())?);
    }
    if !spec.sort.is_empty() {
        find = find.sort(document(spec.sort.clone())?);
    }
    find.run().map_err(server)
}

fn collection(client: &Client, db: &str, name: &str) -> Collection<Document> {
    client.database(db).collection(name)
}

/// The server's version: that the URI reaches it and the login holds.
pub fn ping(client: &Client) -> Result<String, MongoError> {
    let info = client.database("admin").run_command(bson::doc! { "buildInfo": 1 }).run().map_err(server)?;
    Ok(info.get_str("version").unwrap_or("unknown").to_string())
}

pub fn databases(client: &Client) -> Result<Vec<DatabaseInfo>, MongoError> {
    let specs = client.list_databases().run().map_err(server)?;
    Ok(specs.into_iter().map(|spec| DatabaseInfo { name: spec.name, size_on_disk: spec.size_on_disk }).collect())
}

pub fn collections(client: &Client, db: &str) -> Result<Vec<CollectionInfo>, MongoError> {
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

/// Every match counted, or — with no filter — the collection's own count
/// from its metadata, which does not read it.
pub fn count(client: &Client, db: &str, name: &str, filter: Map<String, Value>) -> Result<u64, MongoError> {
    let collection = collection(client, db, name);
    if filter.is_empty() {
        return collection.estimated_document_count().max_time(QUERY_TIMEOUT).run().map_err(server);
    }
    collection.count_documents(document(filter)?).max_time(QUERY_TIMEOUT).run().map_err(server)
}

/// The first `cap` results: a pipeline can return the whole collection.
pub fn aggregate(client: &Client, db: &str, name: &str, pipeline: Vec<Map<String, Value>>, cap: usize) -> Result<Vec<Value>, MongoError> {
    let stages = pipeline.into_iter().map(document).collect::<Result<Vec<_>, _>>()?;
    collection(client, db, name).aggregate(stages).max_time(QUERY_TIMEOUT).run().map_err(server)?.take(cap).map(|doc| doc.map(canonical).map_err(server)).collect()
}

/// How many went in. In order, stopping at the first the server refuses: the
/// ones before it are in, which the error says. All in but the write concern
/// not met, they stay in, and the error says that.
pub fn insert(client: &Client, db: &str, name: &str, documents: Vec<Map<String, Value>>) -> Result<usize, MongoError> {
    let documents = documents.into_iter().map(document).collect::<Result<Vec<_>, _>>()?;
    let total = documents.len();
    match collection(client, db, name).insert_many(documents).run() {
        Ok(inserted) => Ok(inserted.inserted_ids.len()),
        Err(error) => Err(match error.kind.as_ref() {
            mongodb::error::ErrorKind::InsertMany(mongodb::error::InsertManyError { write_errors: Some(refused), .. }) if !refused.is_empty() => {
                let first = &refused[0];
                // A write error comes with its code, its name seldom.
                let reason = format!("{} ({})", first.message, first.code_name.clone().unwrap_or_else(|| format!("code {}", first.code)));
                match first.index {
                    0 => MongoError::Server(reason),
                    inserted => MongoError::PartlyInserted { inserted, total, reason },
                }
            }
            mongodb::error::ErrorKind::InsertMany(mongodb::error::InsertManyError { write_concern_error: Some(concern), .. }) => {
                MongoError::Unconfirmed { inserted: total, reason: format!("{} ({})", concern.message, concern.code_name) }
            }
            _ => server(error),
        }),
    }
}

/// The document with `id` replaced by `replacement`.
pub fn replace(client: &Client, db: &str, name: &str, id: Value, replacement: Map<String, Value>) -> Result<(), MongoError> {
    let id = bson_of(id)?;
    let replacement = document(replacement)?;
    // Without one it keeps its own; with another, `replace_one` would fail
    // on the server with a message about an immutable field.
    if replacement.get("_id").is_some_and(|given| *given != id) {
        return Err(MongoError::IdChanged);
    }
    let replaced = collection(client, db, name).replace_one(bson::doc! { "_id": id }, replacement).run().map_err(server)?;
    if replaced.matched_count == 0 {
        return Err(MongoError::Gone);
    }
    Ok(())
}

/// The document with `id` changed by `update`, an operator document — `{ $set: … }`.
pub fn update(client: &Client, db: &str, name: &str, id: Value, update: Map<String, Value>) -> Result<(), MongoError> {
    let updated = collection(client, db, name).update_one(bson::doc! { "_id": bson_of(id)? }, document(update)?).run().map_err(server)?;
    if updated.matched_count == 0 {
        return Err(MongoError::Gone);
    }
    Ok(())
}

pub fn delete(client: &Client, db: &str, name: &str, id: Value) -> Result<(), MongoError> {
    let deleted = collection(client, db, name).delete_one(bson::doc! { "_id": bson_of(id)? }).run().map_err(server)?;
    if deleted.deleted_count == 0 {
        return Err(MongoError::Gone);
    }
    Ok(())
}

pub fn indexes(client: &Client, db: &str, name: &str) -> Result<Vec<IndexView>, MongoError> {
    collection(client, db, name)
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
pub fn create_index(client: &Client, db: &str, name: &str, keys: Map<String, Value>, unique: bool, index: Option<String>) -> Result<String, MongoError> {
    let options = IndexOptions::builder().unique(unique.then_some(true)).name(index).build();
    let model = IndexModel::builder().keys(document(keys)?).options(options).build();
    Ok(collection(client, db, name).create_index(model).run().map_err(server)?.index_name)
}

pub fn drop_index(client: &Client, db: &str, name: &str, index: &str) -> Result<(), MongoError> {
    collection(client, db, name).drop_index(index).run().map_err(server)
}

/// In a database not there yet, it makes the database too.
pub fn create_collection(client: &Client, db: &str, name: &str) -> Result<(), MongoError> {
    client.database(db).create_collection(name).run().map_err(server)
}

pub fn drop_collection(client: &Client, db: &str, name: &str) -> Result<(), MongoError> {
    collection(client, db, name).drop().run().map_err(server)
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
    /// A page's size.
    pub limit: usize,
}

/// MaxTimeMSExpired: the server stopped the query at its `max_time`.
const MAX_TIME_EXPIRED: i32 = 50;
/// CursorNotFound: the server let an idle cursor go.
const CURSOR_NOT_FOUND: i32 = 43;

fn cursor_gone(error: &mongodb::error::Error) -> bool {
    matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(refused) if refused.code == CURSOR_NOT_FOUND)
}

/// A connection string the driver could not read. Not in its words: they
/// repeat the part it stopped at — an option's value, a password's tail.
fn unread(error: mongodb::error::Error) -> MongoError {
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::InvalidArgument { .. } => {
            MongoError::BadUri("the driver cannot read this connection string — check its hosts and options, and percent-encode / ? # [ ] @ : & in a password or an option's value".into())
        }
        // A `mongodb+srv://` lookup that failed names the host, not the password.
        _ => server(error),
    }
}

/// A command the server refused says what it said, not the driver's dump of its reply.
fn server(error: mongodb::error::Error) -> MongoError {
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::Command(refused) if refused.code == MAX_TIME_EXPIRED => MongoError::TimedOut,
        mongodb::error::ErrorKind::Command(refused) => MongoError::Server(format!("{} ({})", refused.message, refused.code_name)),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteConcernError(unmet)) => MongoError::Server(format!("the write concern was not met: {} ({})", unmet.message, unmet.code_name)),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(refused)) => {
            MongoError::Server(format!("{} ({})", refused.message, refused.code_name.clone().unwrap_or_else(|| format!("code {}", refused.code))))
        }
        _ => MongoError::Server(error.to_string()),
    }
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

    /// A one-member replica set at `KIBO_MONGO_RS_URI` (default port 27018), asked
    /// for two members: the documents go in and the write concern is not met.
    #[test]
    #[ignore = "needs a one-member replica set — set KIBO_MONGO_RS_URI"]
    fn live_server_unconfirmed() {
        let uri = std::env::var("KIBO_MONGO_RS_URI").unwrap_or_else(|_| "mongodb://localhost:27018/?directConnection=true".into());
        let client = open(&SecretString::from(format!("{uri}{}w=2&wtimeoutMS=500", if uri.contains('?') { '&' } else { '?' }))).unwrap();
        let (db, coll) = ("kibo_live_test", format!("w{}", std::process::id()));
        let docs = crate::domain::mongo::parse_documents("[{ _id: 1 }, { _id: 2 }]", "d").unwrap();
        let unconfirmed = insert(&client, db, &coll, docs);
        assert!(matches!(&unconfirmed, Err(MongoError::Unconfirmed { inserted: 2, reason }) if reason.contains("UnsatisfiableWriteConcern")), "{unconfirmed:?}");
        assert_eq!(count(&client, db, &coll, Map::new()).unwrap(), 2, "they stay in");
        // Any other write: what the server said, not the driver's dump of its reply.
        let unmet = drop_collection(&client, db, &coll);
        assert!(matches!(&unmet, Err(MongoError::Server(m)) if m == "the write concern was not met: Not enough data-bearing nodes (UnsatisfiableWriteConcern)"), "{unmet:?}");
        let gone = delete(&client, db, &coll, json!(1));
        assert!(matches!(&gone, Err(MongoError::Server(m)) if m.starts_with("the write concern was not met")), "{gone:?}");
        drop_collection(&open(&SecretString::from(uri)).unwrap(), db, &coll).unwrap();
    }

    /// A real server, at `KIBO_MONGO_URI` (default `mongodb://localhost:27017`):
    /// a scratch collection made, written, read, indexed and dropped.
    #[test]
    #[ignore = "needs a MongoDB server — set KIBO_MONGO_URI"]
    fn live_server() {
        let uri = SecretString::from(std::env::var("KIBO_MONGO_URI").unwrap_or_else(|_| "mongodb://localhost:27017".into()));
        let clients = MongoClients::default();
        let client = clients.client("live", || Ok(uri.clone())).unwrap();
        let place = || Place { connection: "live", client: &client };
        let db = "kibo_live_test";
        let coll = format!("c{}", std::process::id());
        let version = ping(&client).unwrap();
        assert!(!version.is_empty());

        create_collection(&client, db, &coll).unwrap();
        assert!(collections(&client, db).unwrap().iter().any(|c| c.name == coll && c.kind == "collection"));
        assert!(databases(&client).unwrap().iter().any(|d| d.name == db));

        let docs = crate::domain::mongo::parse_documents("[{ _id: 1, n: NumberLong(5), name: 'a' }, { _id: 2, name: 'b' }, { _id: 3, name: 'c' }]", "the documents").unwrap();
        assert_eq!(insert(&client, db, &coll, docs).unwrap(), 3);
        assert_eq!(count(&client, db, &coll, Map::new()).unwrap(), 3);
        // In order, up to the first refused: what went in before it is said, not a dump of the reply.
        let partly = crate::domain::mongo::parse_documents("[{ _id: 20 }, { _id: 1 }, { _id: 21 }]", "d").unwrap();
        let refused = insert(&client, db, &coll, partly);
        assert!(matches!(&refused, Err(MongoError::PartlyInserted { inserted: 1, total: 3, reason }) if reason.ends_with("(code 11000)")), "{refused:?}");
        let first = crate::domain::mongo::parse_documents("[{ _id: 1 }]", "d").unwrap();
        assert!(matches!(insert(&client, db, &coll, first), Err(MongoError::Server(m)) if m.starts_with("E11000") && m.ends_with("(code 11000)")));
        delete(&client, db, &coll, json!(20)).unwrap();
        assert_eq!(count(&client, db, &coll, parse_document("{ name: /^[ab]$/ }", "f").unwrap()).unwrap(), 2);

        let spec = FindSpec { sort: parse_document("{ _id: -1 }", "s").unwrap(), projection: parse_document("{ name: 1 }", "p").unwrap(), limit: 2, skip: 0, ..Default::default() };
        let found = clients.find(place(), db, &coll, spec).unwrap().documents;
        assert_eq!(found, vec![json!({ "_id": { "$numberInt": "3" }, "name": "c" }), json!({ "_id": { "$numberInt": "2" }, "name": "b" })]);

        let first = clients.find(place(), db, &coll, FindSpec { filter: parse_document("{ _id: 1 }", "f").unwrap(), limit: 1, ..Default::default() }).unwrap().documents;
        assert_eq!(first[0]["n"], json!({ "$numberLong": "5" }), "an Int64 comes back as one");

        replace(&client, db, &coll, json!(1), parse_document("{ _id: 1, name: 'z' }", "d").unwrap()).unwrap();
        assert!(matches!(replace(&client, db, &coll, json!(1), parse_document("{ _id: 9 }", "d").unwrap()), Err(MongoError::IdChanged)));
        let one = |keys: &[&str], text: &str| {
            crate::domain::mongo::set_fields(&[crate::domain::mongo::FieldChange { path: keys.iter().map(|k| k.to_string()).collect(), text: text.into() }]).unwrap()
        };
        update(&client, db, &coll, json!(3), one(&["tags"], "['x', 'y']")).unwrap();
        update(&client, db, &coll, json!(3), one(&["tags", "1"], "NumberLong(7)")).unwrap();
        let third = clients.find(place(), db, &coll, FindSpec { filter: parse_document("{ _id: 3 }", "f").unwrap(), limit: 1, ..Default::default() }).unwrap().documents;
        assert_eq!(third[0]["tags"], json!(["x", { "$numberLong": "7" }]), "one item of an array set, its type kept");
        assert!(matches!(update(&client, db, &coll, json!(99), one(&["name"], "'q'")), Err(MongoError::Gone)));
        let grouped = aggregate(&client, db, &coll, crate::domain::mongo::parse_pipeline("[{ $match: { name: 'z' } }, { $project: { _id: 0, name: 1 } }]").unwrap(), 10).unwrap();
        assert_eq!(grouped, vec![json!({ "name": "z" })]);
        assert_eq!(aggregate(&client, db, &coll, Vec::new(), 2).unwrap().len(), 2, "capped");

        let name = create_index(&client, db, &coll, parse_document("{ name: 1 }", "k").unwrap(), true, None).unwrap();
        let indexes = indexes(&client, db, &coll).unwrap();
        let made = indexes.iter().find(|i| i.name == name).unwrap();
        assert!(made.unique);
        assert_eq!(made.keys, "{ name: 1 }");
        drop_index(&client, db, &coll, &name).unwrap();

        delete(&client, db, &coll, json!(2)).unwrap();
        assert!(matches!(delete(&client, db, &coll, json!(2)), Err(MongoError::Gone)));
        assert!(matches!(replace(&client, db, &coll, json!(2), Map::new()), Err(MongoError::Gone)));

        // A view lists as one and is found like a collection; counting what takes too long stops as `TimedOut`.
        let view = format!("{coll}_view");
        client.database(db).create_collection(&view).view_on(coll.clone()).pipeline([bson::doc! { "$match": { "name": { "$exists": true } } }]).run().unwrap();
        assert!(collections(&client, db).unwrap().iter().any(|c| c.name == view && c.kind == "view"));
        assert_eq!(clients.find(place(), db, &view, FindSpec { limit: 5, ..Default::default() }).unwrap().documents.len(), 2);
        assert_eq!(count(&client, db, &view, Map::new()).unwrap(), 2);
        let slow = document(parse_document("{ $expr: { $function: { body: 'function() { sleep(1000); return true; }', args: [], lang: 'js' } } }", "f").unwrap()).unwrap();
        let stopped = client.database(db).collection::<Document>(&view).count_documents(slow).max_time(Duration::from_millis(300)).run().unwrap_err();
        assert!(matches!(server(stopped), MongoError::TimedOut));
        assert!(matches!(clients.find(place(), db, &view, FindSpec { filter: parse_document("{ $nope: 1 }", "f").unwrap(), limit: 1, ..Default::default() }), Err(MongoError::Server(m)) if m.ends_with("(BadValue)") && !m.contains("RawDocumentBuf")));
        drop_collection(&client, db, &view).unwrap();

        // Pages read on from where the last stopped, and on again when the server let the cursor go.
        let more = (10..15).map(|id| format!("{{ _id: {id}, name: 'p' }}")).collect::<Vec<_>>().join(", ");
        insert(&client, db, &coll, crate::domain::mongo::parse_documents(&format!("[{more}]"), "d").unwrap()).unwrap();
        let by_id = |limit: usize| FindSpec { sort: parse_document("{ _id: 1 }", "s").unwrap(), projection: parse_document("{ _id: 1 }", "p").unwrap(), limit, ..Default::default() };
        let ids = |found: &Found| found.documents.iter().map(|d| d["_id"]["$numberInt"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let first = clients.find(place(), db, &coll, by_id(3)).unwrap();
        assert_eq!(ids(&first), ["1", "3", "10"]);
        let idle: Vec<Document> = client
            .database("admin")
            .aggregate([bson::doc! { "$currentOp": { "idleCursors": true } }, bson::doc! { "$match": { "type": "idleCursor", "ns": format!("{db}.{coll}") } }])
            .run()
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let cursors: Vec<i64> = idle.iter().map(|op| op.get_document("cursor").unwrap().get_i64("cursorId").unwrap()).collect();
        assert!(!cursors.is_empty());
        client.database(db).run_command(bson::doc! { "killCursors": &coll, "cursors": cursors }).run().unwrap();
        let second = clients.find_more(first.next.unwrap()).unwrap();
        assert_eq!(ids(&second), ["11", "12", "13"]);
        assert!(matches!(clients.find_more(first.next.unwrap()), Err(MongoError::FindClosed)), "a page is read once");
        let last = clients.find_more(second.next.unwrap()).unwrap();
        assert_eq!((ids(&last), last.next), (vec!["14".to_string()], None));
        let whole = clients.find(place(), db, &coll, by_id(7)).unwrap();
        assert_eq!((whole.documents.len(), whole.next), (7, None), "a page just full is the last");
        let open = clients.find(place(), db, &coll, by_id(3)).unwrap().next.unwrap();
        clients.find_close(open);
        assert!(matches!(clients.find_more(open), Err(MongoError::FindClosed)));
        let oldest = clients.find(place(), db, &coll, by_id(1)).unwrap().next.unwrap();
        let newest = (0..MAX_OPEN_FINDS).map(|_| clients.find(place(), db, &coll, by_id(1)).unwrap().next.unwrap()).last().unwrap();
        assert!(matches!(clients.find_more(oldest), Err(MongoError::FindClosed)), "past the limit, the oldest find is closed");
        assert_eq!(ids(&clients.find_more(newest).unwrap()), ["3"]);

        // A connection forgotten takes its finds with it; another's stay.
        let other = super::open(&uri).unwrap();
        let mine = clients.find(place(), db, &coll, by_id(1)).unwrap().next.unwrap();
        let theirs = clients.find(Place { connection: "other", client: &other }, db, &coll, by_id(1)).unwrap().next.unwrap();
        clients.forget("live");
        assert!(matches!(clients.find_more(mine), Err(MongoError::FindClosed)));
        assert_eq!(ids(&clients.find_more(theirs).unwrap()), ["3"]);

        drop_collection(&client, db, &coll).unwrap();
        assert!(!collections(&client, db).unwrap().iter().any(|c| c.name == coll));
    }

    #[test]
    fn a_string_the_driver_cannot_read_is_refused_without_repeating_it() {
        for uri in ["mongodb://h/?authMechanismProperties=AWS_SESSION_TOKEN=hunter2", "mongodb://h/?tlsCertificateKeyFilePassword=pa&hunter2", "mongodb://u:p/hunter2@h", "mongodb://h/?tlsCertificateKeyFilePassword=pa&hunter2=x"] {
            let refused = open(&SecretString::from(uri)).unwrap_err();
            assert!(matches!(&refused, MongoError::BadUri(m) if !m.contains("hunter2")), "{uri}: {refused}");
            let refused = check(&SecretString::from(uri)).unwrap_err();
            assert!(matches!(&refused, MongoError::BadUri(m) if !m.contains("hunter2")), "{uri}: {refused}");
        }
        assert!(check(&SecretString::from("mongodb+srv://c.example.net/?w=1")).is_ok(), "read, not looked up");
    }

    /// Made once per connection, so a call does not open the sealed store;
    /// made again from the saved URI once the connection changed.
    #[test]
    fn a_client_is_kept_per_connection_until_forgotten() {
        let clients = MongoClients::default();
        let uri = || Ok(SecretString::from("mongodb://127.0.0.1:1"));
        let unread = || Err(MongoError::NoName);
        clients.client("a", uri).unwrap();
        clients.client("b", uri).unwrap();
        assert!(clients.client("a", unread).is_ok(), "kept: the URI is not read again");
        clients.forget("a");
        assert!(clients.client("a", unread).is_err(), "forgotten: read again");
        assert!(clients.client("b", unread).is_ok(), "the others stay");
        clients.forget_all();
        assert!(clients.client("b", unread).is_err());
        assert!(matches!(clients.client("c", || Ok(SecretString::from("mongodb://"))), Err(MongoError::BadUri(_))));
        assert!(clients.client("c", unread).is_err(), "a URI that did not open is not kept");
    }

    #[test]
    #[ignore = "waits for the server selection timeout"]
    fn an_unreachable_server_says_so_in_seconds() {
        let started = std::time::Instant::now();
        let error = ping(&open(&SecretString::from("mongodb://127.0.0.1:1")).unwrap()).unwrap_err();
        assert!(started.elapsed() < SELECT_TIMEOUT * 2, "{:?}", started.elapsed());
        assert!(matches!(error, MongoError::Server(_)), "{error}");
    }
}
