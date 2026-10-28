//! The MongoDB page's use cases (`docs/44-mongodb.md`): a connection saved,
//! tested and forgotten; a collection browsed, queried and edited. The page's
//! text is read here (`domain::mongo`), the URI comes from the sealed store,
//! and the driver gets both — the window never sees a URI, and the driver
//! never sees the window's text.
//!
//! The model's tools, when they come (`docs/44-mongodb.md`), go through these
//! same functions.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::domain::mongo::{self, Connection, ConnectionDraft, ConnectionView, FieldChange, IndexView, MongoError};
use crate::infra::mongo_client::{self, CollectionInfo, DatabaseInfo, FindSpec, Found, MongoClients, Place};
use crate::infra::mongo_store;

/// Most documents a page of a find shows.
pub const MAX_PAGE: i64 = 500;
/// Most results an aggregation shows: a pipeline may return the collection.
pub const MAX_AGGREGATE: usize = 200;

/// A collection, by the connection it is reached through.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub connection: String,
    pub database: String,
    pub collection: String,
}

/// The page's query bar as typed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindRequest {
    pub filter: String,
    pub projection: String,
    pub sort: String,
    pub skip: u64,
    pub limit: i64,
}

/// One document as the page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentView {
    /// Its `_id` in canonical Extended JSON, sent back as it came to edit or
    /// delete it; `None` for a result with none (an aggregation's).
    pub id: Option<Value>,
    /// As mongosh shows it, and as the editor takes it back.
    pub text: String,
    /// Field by field, as the card draws it.
    pub fields: Vec<(String, mongo::Shown)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindPage {
    pub documents: Vec<DocumentView>,
    /// Where the next page is read from (`find_more`) — `None` after the last.
    /// No total: on a view, counting runs its whole pipeline; `count` asks for it.
    pub next: Option<u64>,
}

pub fn connections() -> Result<Vec<ConnectionView>, MongoError> {
    mongo_store::list()
}

/// A new connection, or a saved one changed: its name, and its URI when one
/// was typed — then its client is made again from it. The list after it.
pub fn save_connection(clients: &MongoClients, draft: ConnectionDraft) -> Result<Vec<ConnectionView>, MongoError> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(MongoError::NoName);
    }
    let uri = draft.uri.as_deref().map(str::trim).filter(|uri| !uri.is_empty());
    match (draft.id, uri) {
        (Some(id), None) => mongo_store::rename(&id, name)?,
        (id, Some(uri)) => {
            mongo::validate_uri(uri)?;
            // Read once as the driver reads it: an option it does not know is a value's tail, which the mask would show.
            mongo_client::check(&secrecy::SecretString::from(uri))?;
            let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            mongo_store::put(Connection { id: id.clone(), name: name.to_string(), uri: uri.to_string() })?;
            clients.forget(&id);
        }
        (None, None) => return Err(MongoError::BadUri("a new connection needs its connection string".into())),
    }
    mongo_store::list()
}

pub fn delete_connection(clients: &MongoClients, id: &str) -> Result<Vec<ConnectionView>, MongoError> {
    mongo_store::delete(id)?;
    clients.forget(id);
    mongo_store::list()
}

/// The server's version — with the URI typed in the form, on a client made
/// for the test alone and gone after it, or with the saved one when none was typed.
pub fn test_connection(clients: &MongoClients, draft: ConnectionDraft) -> Result<String, MongoError> {
    let typed = draft.uri.as_deref().map(str::trim).filter(|uri| !uri.is_empty());
    match (typed, draft.id) {
        (Some(uri), _) => {
            mongo::validate_uri(uri)?;
            mongo_client::ping(&mongo_client::open(&secrecy::SecretString::from(uri))?)
        }
        (None, Some(id)) => mongo_client::ping(&client(clients, &id)?),
        (None, None) => Err(MongoError::BadUri("type a connection string to test".into())),
    }
}

/// The saved connection's client: its URI read from the sealed store only the first time.
fn client(clients: &MongoClients, connection: &str) -> Result<mongo_client::Client, MongoError> {
    clients.client(connection, || mongo_store::uri(connection))
}

pub fn databases(clients: &MongoClients, connection: &str) -> Result<Vec<DatabaseInfo>, MongoError> {
    mongo_client::databases(&client(clients, connection)?)
}

pub fn collections(clients: &MongoClients, connection: &str, database: &str) -> Result<Vec<CollectionInfo>, MongoError> {
    mongo_client::collections(&client(clients, connection)?, database)
}

pub fn find(clients: &MongoClients, target: &Target, request: &FindRequest) -> Result<FindPage, MongoError> {
    let spec = FindSpec {
        filter: mongo::parse_document(&request.filter, "the filter")?,
        projection: mongo::parse_document(&request.projection, "the projection")?,
        sort: mongo::parse_document(&request.sort, "the sort")?,
        skip: request.skip,
        limit: request.limit.clamp(1, MAX_PAGE) as usize,
    };
    let client = client(clients, &target.connection)?;
    Ok(page(clients.find(Place { connection: &target.connection, client: &client }, &target.database, &target.collection, spec)?))
}

/// The page after the one that came with `next`.
pub fn find_more(clients: &MongoClients, next: u64) -> Result<FindPage, MongoError> {
    Ok(page(clients.find_more(next)?))
}

/// Every document the filter matches.
pub fn count(clients: &MongoClients, target: &Target, filter: &str) -> Result<u64, MongoError> {
    let filter = mongo::parse_document(filter, "the filter")?;
    mongo_client::count(&client(clients, &target.connection)?, &target.database, &target.collection, filter)
}

fn page(found: Found) -> FindPage {
    FindPage { documents: found.documents.iter().map(view).collect(), next: found.next }
}

/// What a pipeline returned — or, for one that writes and was not said to, the
/// stage that would, with nothing run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Aggregated {
    pub documents: Vec<DocumentView>,
    pub writes: Option<String>,
}

/// `writes`: run it even when a stage writes into a collection — the user was asked.
pub fn aggregate(clients: &MongoClients, target: &Target, pipeline: &str, writes: bool) -> Result<Aggregated, MongoError> {
    let stages = mongo::parse_pipeline(pipeline)?;
    if let Some(stage) = mongo::writing_stage(&stages).filter(|_| !writes) {
        return Ok(Aggregated { documents: Vec::new(), writes: Some(stage.to_string()) });
    }
    let documents = mongo_client::aggregate(&client(clients, &target.connection)?, &target.database, &target.collection, stages, MAX_AGGREGATE)?;
    Ok(Aggregated { documents: documents.iter().map(view).collect(), writes: None })
}

/// What an insert did: how many went in, and — refused part way — why, and the
/// documents still to go as text for the editor, so trying again does not
/// insert the first ones twice. All in but unconfirmed: why, and nothing to go.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inserted {
    pub count: usize,
    pub refused: Option<String>,
    pub rest: Option<String>,
}

pub fn insert(clients: &MongoClients, target: &Target, text: &str) -> Result<Inserted, MongoError> {
    let documents = mongo::parse_documents(text, "the documents")?;
    let kept = documents.clone();
    outcome(mongo_client::insert(&client(clients, &target.connection)?, &target.database, &target.collection, documents), kept)
}

/// A refusal after some went in is not a failure of the call: the window is told what is in, and what is still to go.
fn outcome(inserted: Result<usize, MongoError>, kept: Vec<Map<String, Value>>) -> Result<Inserted, MongoError> {
    match inserted {
        Ok(count) => Ok(Inserted { count, refused: None, rest: None }),
        Err(refused @ MongoError::PartlyInserted { inserted, .. }) => Ok(refused_after(kept, inserted, refused.to_string())),
        Err(unconfirmed @ MongoError::Unconfirmed { inserted, .. }) => Ok(Inserted { count: inserted, refused: Some(unconfirmed.to_string()), rest: None }),
        Err(error) => Err(error),
    }
}

/// The documents from the one refused on, written as they are typed: one alone, or an array.
fn refused_after(documents: Vec<Map<String, Value>>, inserted: usize, reason: String) -> Inserted {
    let mut rest: Vec<Value> = documents.into_iter().skip(inserted).map(Value::Object).collect();
    let rest = if rest.len() == 1 { rest.remove(0) } else { Value::Array(rest) };
    Inserted { count: inserted, refused: Some(reason), rest: Some(mongo::to_shell(&rest)) }
}

pub fn replace(clients: &MongoClients, target: &Target, id: Value, text: &str) -> Result<(), MongoError> {
    let replacement = mongo::parse_document(text, "the document")?;
    if replacement.is_empty() {
        return Err(MongoError::WrongShape { what: "the document", expected: "a document with its fields — delete it to remove it" });
    }
    mongo_client::replace(&client(clients, &target.connection)?, &target.database, &target.collection, id, replacement)
}

pub fn set_fields(clients: &MongoClients, target: &Target, id: Value, changes: &[FieldChange]) -> Result<(), MongoError> {
    let update = mongo::set_fields(changes)?;
    mongo_client::update(&client(clients, &target.connection)?, &target.database, &target.collection, id, update)
}

pub fn delete(clients: &MongoClients, target: &Target, id: Value) -> Result<(), MongoError> {
    mongo_client::delete(&client(clients, &target.connection)?, &target.database, &target.collection, id)
}

pub fn indexes(clients: &MongoClients, target: &Target) -> Result<Vec<IndexView>, MongoError> {
    mongo_client::indexes(&client(clients, &target.connection)?, &target.database, &target.collection)
}

/// The new index's name.
pub fn create_index(clients: &MongoClients, target: &Target, keys: &str, unique: bool, name: Option<String>) -> Result<String, MongoError> {
    let keys = mongo::parse_document(keys, "the keys")?;
    if keys.is_empty() {
        return Err(MongoError::WrongShape { what: "the keys", expected: "a field and its order, { name: 1 }" });
    }
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    mongo_client::create_index(&client(clients, &target.connection)?, &target.database, &target.collection, keys, unique, name)
}

pub fn drop_index(clients: &MongoClients, target: &Target, name: &str) -> Result<(), MongoError> {
    mongo_client::drop_index(&client(clients, &target.connection)?, &target.database, &target.collection, name)
}

/// In a database not there yet, it makes that too: MongoDB has no empty database.
pub fn create_collection(clients: &MongoClients, target: &Target) -> Result<(), MongoError> {
    let (database, collection) = (target.database.trim(), target.collection.trim());
    if database.is_empty() || collection.is_empty() {
        return Err(MongoError::WrongShape { what: "a new collection", expected: "named, in a named database" });
    }
    mongo_client::create_collection(&client(clients, &target.connection)?, database, collection)
}

pub fn drop_collection(clients: &MongoClients, target: &Target) -> Result<(), MongoError> {
    mongo_client::drop_collection(&client(clients, &target.connection)?, &target.database, &target.collection)
}

fn view(document: &Value) -> DocumentView {
    DocumentView {
        id: document.get("_id").cloned(),
        text: mongo::to_shell(document),
        fields: document.as_object().map(mongo::fields).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use serde_json::json;

    fn draft(id: Option<&str>, name: &str, uri: Option<&str>) -> ConnectionDraft {
        ConnectionDraft { id: id.map(str::to_string), name: name.into(), uri: uri.map(str::to_string) }
    }

    #[test]
    fn a_connection_is_added_renamed_given_a_new_uri_and_deleted() {
        with_app_dir("mongo-service-connections", || {
            let clients = MongoClients::default();
            let list = save_connection(&clients, draft(None, " Local ", Some(" mongodb://u:pw@localhost "))).unwrap();
            assert_eq!(list.len(), 1);
            let id = list[0].id.clone();
            assert_eq!((list[0].name.as_str(), list[0].address.as_str()), ("Local", "mongodb://u:•••@localhost"));
            // Whether its client is kept: one made again would ask for the URI.
            let kept = || clients.client(&id, || Err(MongoError::NoName)).is_ok();
            client(&clients, &id).unwrap();
            assert!(kept());

            // An edit with the URI left empty keeps it, and the client made from it.
            let list = save_connection(&clients, draft(Some(&id), "Dev", Some("  "))).unwrap();
            assert_eq!((list[0].name.as_str(), list[0].address.as_str()), ("Dev", "mongodb://u:•••@localhost"));
            assert!(kept());
            let list = save_connection(&clients, draft(Some(&id), "Dev", Some("mongodb+srv://c.example.net"))).unwrap();
            assert_eq!((list.len(), list[0].address.as_str()), (1, "mongodb+srv://c.example.net"));
            assert!(!kept(), "a new URI, a new client");

            // Back to a host the test does not look up, its client made again.
            save_connection(&clients, draft(Some(&id), "Dev", Some("mongodb://h2"))).unwrap();
            client(&clients, &id).unwrap();
            let other = save_connection(&clients, draft(None, "Other", Some("mongodb://h"))).unwrap()[1].id.clone();
            client(&clients, &other).unwrap();
            assert_eq!(delete_connection(&clients, &id).unwrap().len(), 1);
            assert!(!kept(), "a deleted connection's client goes");
            assert!(clients.client(&other, || Err(MongoError::NoName)).is_ok(), "the others' stay");
        });
    }

    #[test]
    fn a_draft_without_what_it_needs_is_refused_before_anything_is_saved() {
        with_app_dir("mongo-service-refused", || {
            assert!(matches!(save_connection(&MongoClients::default(), draft(None, " ", Some("mongodb://h"))), Err(MongoError::NoName)));
            assert!(matches!(save_connection(&MongoClients::default(), draft(None, "A", None)), Err(MongoError::BadUri(_))));
            assert!(matches!(save_connection(&MongoClients::default(), draft(None, "A", Some("http://h"))), Err(MongoError::BadUri(_))));
            let password_cut = save_connection(&MongoClients::default(), draft(None, "A", Some("mongodb://h/?tlsCertificateKeyFilePassword=pa&hunter2=x")));
            assert!(matches!(&password_cut, Err(MongoError::BadUri(m)) if !m.contains("hunter2")), "{password_cut:?}");
            assert!(matches!(save_connection(&MongoClients::default(), draft(Some("gone"), "A", None)), Err(MongoError::NoConnection(_))));
            assert!(connections().unwrap().is_empty());
            let clients = MongoClients::default();
            assert!(matches!(test_connection(&clients, draft(None, "", None)), Err(MongoError::BadUri(_))));
            assert!(matches!(test_connection(&clients, draft(None, "", Some("localhost"))), Err(MongoError::BadUri(_))));
            assert!(matches!(test_connection(&clients, draft(Some("gone"), "", None)), Err(MongoError::NoConnection(_))));
        });
    }

    /// The query bar's text is read before the server is asked: a mistake is
    /// the page's to show, with where it is.
    #[test]
    fn a_bad_query_is_refused_with_its_place_before_the_server_is_asked() {
        with_app_dir("mongo-service-query", || {
            let id = save_connection(&MongoClients::default(), draft(None, "A", Some("mongodb://127.0.0.1:1"))).unwrap()[0].id.clone();
            let target = Target { connection: id, database: "d".into(), collection: "c".into() };
            let clients = MongoClients::default();
            let request = FindRequest { filter: "{ a: }".into(), ..Default::default() };
            let refused = find(&clients, &target, &request).unwrap_err().to_string();
            assert!(refused.starts_with("the filter: line 1"), "{refused}");
            let request = FindRequest { sort: "[1]".into(), ..Default::default() };
            assert!(find(&clients, &target, &request).unwrap_err().to_string().starts_with("the sort must be"));
            assert!(aggregate(&clients, &target, "[1]", false).unwrap_err().to_string().starts_with("the pipeline must be"));
            // One that writes waits to be said so, and no server is asked; said so, it goes on — here to a connection not there.
            let gone = Target { connection: "gone".into(), ..target.clone() };
            assert_eq!(aggregate(&clients, &gone, "[{ $match: {} }, { $out: 'copy' }]", false).unwrap(), Aggregated { documents: Vec::new(), writes: Some("$out".into()) });
            assert!(matches!(aggregate(&clients, &gone, "{ $merge: 'copy' }", true), Err(MongoError::NoConnection(_))));
            assert!(matches!(aggregate(&clients, &gone, "[{ $match: {} }]", false), Err(MongoError::NoConnection(_))), "one that only reads runs");
            assert!(insert(&clients, &target, "[]").unwrap_err().to_string().starts_with("the documents must be"));
            assert!(replace(&clients, &target, json!(1), "{}").unwrap_err().to_string().contains("delete it"));
            assert!(create_index(&clients, &target, "", false, None).unwrap_err().to_string().starts_with("the keys must be"));
            let unnamed = Target { collection: " ".into(), ..target.clone() };
            assert!(create_collection(&clients, &unnamed).is_err());
        });
    }

    #[test]
    fn an_insert_refused_part_way_leaves_the_rest_to_try_again() {
        let documents = mongo::parse_documents("[{ name: 'a' }, { name: 'b', n: NumberLong(2) }, { name: 'c' }]", "d").unwrap();
        let after = outcome(Err(MongoError::PartlyInserted { inserted: 1, total: 3, reason: "dup".into() }), documents.clone()).unwrap();
        assert_eq!((after.count, after.refused.as_deref()), (1, Some("1 of 3 went in before one was refused, and stay in: dup")));
        assert_eq!(after.rest.as_deref(), Some("[\n  {\n    name: \"b\",\n    n: NumberLong(2)\n  },\n  {\n    name: \"c\"\n  }\n]"));
        assert_eq!(refused_after(documents, 2, String::new()).rest.as_deref(), Some("{\n  name: \"c\"\n}"), "one left is one document");
        // All in, the write concern not met: they stay in, and nothing is left to insert again.
        let unconfirmed = outcome(Err(MongoError::Unconfirmed { inserted: 3, reason: "w".into() }), Vec::new()).unwrap();
        assert_eq!((unconfirmed.count, unconfirmed.rest), (3, None));
        assert!(unconfirmed.refused.is_some_and(|m| m.starts_with("3 went in, but")));
        assert!(matches!(outcome(Err(MongoError::Gone), Vec::new()), Err(MongoError::Gone)), "none went in: the call failed");
        assert_eq!(outcome(Ok(2), Vec::new()).unwrap(), Inserted { count: 2, refused: None, rest: None });
    }

    #[test]
    fn a_document_shows_as_mongosh_text_with_its_id_kept_for_the_way_back() {
        let shown = view(&json!({ "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" }, "n": { "$numberLong": "7" } }));
        assert_eq!(shown.id, Some(json!({ "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" })));
        assert_eq!(shown.text, "{\n  _id: ObjectId(\"65f0a1b2c3d4e5f6a7b8c9d0\"),\n  n: NumberLong(7)\n}");
        assert_eq!(view(&json!({ "total": 3 })).id, None);
    }
}
