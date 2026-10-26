//! The MongoDB page's use cases (`docs/43-mongodb.md`): a connection saved,
//! tested and forgotten; a collection browsed, queried and edited. The page's
//! text is read here (`domain::mongo`), the URI comes from the sealed store,
//! and the driver gets both — the window never sees a URI, and the driver
//! never sees the window's text.
//!
//! The model's tools, when they come (`domain::mongo_tools`), go through these
//! same functions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::mongo::{self, Connection, ConnectionDraft, ConnectionView, MongoError};
use crate::infra::mongo_client::{CollectionInfo, DatabaseInfo, FindSpec, MongoClients};
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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindPage {
    pub documents: Vec<DocumentView>,
    /// Every match, not only this page's.
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexView {
    pub name: String,
    /// `{ name: 1, at: -1 }`
    pub keys: String,
    pub unique: bool,
    pub sparse: bool,
    pub ttl_seconds: Option<u64>,
    pub partial: Option<String>,
}

pub fn connections() -> Result<Vec<ConnectionView>, MongoError> {
    mongo_store::list()
}

/// A new connection, or a saved one changed: its name, and its URI when one
/// was typed. The list after it.
pub fn save_connection(draft: ConnectionDraft) -> Result<Vec<ConnectionView>, MongoError> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(MongoError::NoName);
    }
    let uri = draft.uri.as_deref().map(str::trim).filter(|uri| !uri.is_empty());
    match (draft.id, uri) {
        (Some(id), None) => mongo_store::rename(&id, name)?,
        (id, Some(uri)) => {
            mongo::validate_uri(uri)?;
            let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            mongo_store::put(Connection { id, name: name.to_string(), uri: uri.to_string() })?;
        }
        (None, None) => return Err(MongoError::BadUri("a new connection needs its connection string".into())),
    }
    mongo_store::list()
}

pub fn delete_connection(id: &str) -> Result<Vec<ConnectionView>, MongoError> {
    mongo_store::delete(id)?;
    mongo_store::list()
}

/// The server's version — with the URI typed in the form, or the saved one
/// when none was typed.
pub fn test_connection(clients: &MongoClients, draft: ConnectionDraft) -> Result<String, MongoError> {
    let typed = draft.uri.as_deref().map(str::trim).filter(|uri| !uri.is_empty());
    let uri = match (typed, draft.id) {
        (Some(uri), _) => {
            mongo::validate_uri(uri)?;
            secrecy::SecretString::from(uri)
        }
        (None, Some(id)) => mongo_store::uri(&id)?,
        (None, None) => return Err(MongoError::BadUri("type a connection string to test".into())),
    };
    clients.ping(&uri)
}

pub fn databases(clients: &MongoClients, connection: &str) -> Result<Vec<DatabaseInfo>, MongoError> {
    clients.databases(&mongo_store::uri(connection)?)
}

pub fn collections(clients: &MongoClients, connection: &str, database: &str) -> Result<Vec<CollectionInfo>, MongoError> {
    clients.collections(&mongo_store::uri(connection)?, database)
}

pub fn find(clients: &MongoClients, target: &Target, request: &FindRequest) -> Result<FindPage, MongoError> {
    let uri = mongo_store::uri(&target.connection)?;
    let filter = mongo::parse_document(&request.filter, "the filter")?;
    let spec = FindSpec {
        filter: filter.clone(),
        projection: mongo::parse_document(&request.projection, "the projection")?,
        sort: mongo::parse_document(&request.sort, "the sort")?,
        skip: request.skip,
        limit: request.limit.clamp(1, MAX_PAGE),
    };
    let documents = clients.find(&uri, &target.database, &target.collection, spec)?;
    let total = clients.count(&uri, &target.database, &target.collection, filter)?;
    Ok(FindPage { documents: documents.iter().map(view).collect(), total })
}

pub fn aggregate(clients: &MongoClients, target: &Target, pipeline: &str) -> Result<Vec<DocumentView>, MongoError> {
    let uri = mongo_store::uri(&target.connection)?;
    let stages = mongo::parse_pipeline(pipeline)?;
    Ok(clients.aggregate(&uri, &target.database, &target.collection, stages, MAX_AGGREGATE)?.iter().map(view).collect())
}

/// How many went in.
pub fn insert(clients: &MongoClients, target: &Target, text: &str) -> Result<usize, MongoError> {
    let uri = mongo_store::uri(&target.connection)?;
    let documents = mongo::parse_documents(text, "the documents")?;
    clients.insert(&uri, &target.database, &target.collection, documents)
}

pub fn replace(clients: &MongoClients, target: &Target, id: Value, text: &str) -> Result<(), MongoError> {
    let uri = mongo_store::uri(&target.connection)?;
    let replacement = mongo::parse_document(text, "the document")?;
    if replacement.is_empty() {
        return Err(MongoError::WrongShape { what: "the document", expected: "a document with its fields — delete it to remove it" });
    }
    clients.replace(&uri, &target.database, &target.collection, id, replacement)
}

pub fn delete(clients: &MongoClients, target: &Target, id: Value) -> Result<(), MongoError> {
    clients.delete(&mongo_store::uri(&target.connection)?, &target.database, &target.collection, id)
}

pub fn indexes(clients: &MongoClients, target: &Target) -> Result<Vec<IndexView>, MongoError> {
    let found = clients.indexes(&mongo_store::uri(&target.connection)?, &target.database, &target.collection)?;
    Ok(found
        .into_iter()
        .map(|index| IndexView {
            name: index.name,
            keys: mongo::to_shell_line(&index.keys),
            unique: index.unique,
            sparse: index.sparse,
            ttl_seconds: index.expire_after_seconds,
            partial: index.partial.as_ref().map(mongo::to_shell_line),
        })
        .collect())
}

/// The new index's name.
pub fn create_index(clients: &MongoClients, target: &Target, keys: &str, unique: bool, name: Option<String>) -> Result<String, MongoError> {
    let uri = mongo_store::uri(&target.connection)?;
    let keys = mongo::parse_document(keys, "the keys")?;
    if keys.is_empty() {
        return Err(MongoError::WrongShape { what: "the keys", expected: "a field and its order, { name: 1 }" });
    }
    let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    clients.create_index(&uri, &target.database, &target.collection, keys, unique, name)
}

pub fn drop_index(clients: &MongoClients, target: &Target, name: &str) -> Result<(), MongoError> {
    clients.drop_index(&mongo_store::uri(&target.connection)?, &target.database, &target.collection, name)
}

/// In a database not there yet, it makes that too: MongoDB has no empty database.
pub fn create_collection(clients: &MongoClients, target: &Target) -> Result<(), MongoError> {
    let (database, collection) = (target.database.trim(), target.collection.trim());
    if database.is_empty() || collection.is_empty() {
        return Err(MongoError::WrongShape { what: "a new collection", expected: "named, in a named database" });
    }
    clients.create_collection(&mongo_store::uri(&target.connection)?, database, collection)
}

pub fn drop_collection(clients: &MongoClients, target: &Target) -> Result<(), MongoError> {
    clients.drop_collection(&mongo_store::uri(&target.connection)?, &target.database, &target.collection)
}

fn view(document: &Value) -> DocumentView {
    DocumentView { id: document.get("_id").cloned(), text: mongo::to_shell(document) }
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
            let list = save_connection(draft(None, " Local ", Some(" mongodb://u:pw@localhost "))).unwrap();
            assert_eq!(list.len(), 1);
            let id = list[0].id.clone();
            assert_eq!((list[0].name.as_str(), list[0].address.as_str()), ("Local", "mongodb://u:•••@localhost"));

            // An edit with the URI left empty keeps it.
            let list = save_connection(draft(Some(&id), "Dev", Some("  "))).unwrap();
            assert_eq!((list[0].name.as_str(), list[0].address.as_str()), ("Dev", "mongodb://u:•••@localhost"));
            let list = save_connection(draft(Some(&id), "Dev", Some("mongodb+srv://c.example.net"))).unwrap();
            assert_eq!((list.len(), list[0].address.as_str()), (1, "mongodb+srv://c.example.net"));

            assert!(delete_connection(&id).unwrap().is_empty());
        });
    }

    #[test]
    fn a_draft_without_what_it_needs_is_refused_before_anything_is_saved() {
        with_app_dir("mongo-service-refused", || {
            assert!(matches!(save_connection(draft(None, " ", Some("mongodb://h"))), Err(MongoError::NoName)));
            assert!(matches!(save_connection(draft(None, "A", None)), Err(MongoError::BadUri(_))));
            assert!(matches!(save_connection(draft(None, "A", Some("http://h"))), Err(MongoError::BadUri(_))));
            assert!(matches!(save_connection(draft(Some("gone"), "A", None)), Err(MongoError::NoConnection(_))));
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
            let id = save_connection(draft(None, "A", Some("mongodb://127.0.0.1:1"))).unwrap()[0].id.clone();
            let target = Target { connection: id, database: "d".into(), collection: "c".into() };
            let clients = MongoClients::default();
            let request = FindRequest { filter: "{ a: }".into(), ..Default::default() };
            let refused = find(&clients, &target, &request).unwrap_err().to_string();
            assert!(refused.starts_with("the filter: line 1"), "{refused}");
            let request = FindRequest { sort: "[1]".into(), ..Default::default() };
            assert!(find(&clients, &target, &request).unwrap_err().to_string().starts_with("the sort must be"));
            assert!(aggregate(&clients, &target, "[1]").unwrap_err().to_string().starts_with("the pipeline must be"));
            assert!(insert(&clients, &target, "[]").unwrap_err().to_string().starts_with("the documents must be"));
            assert!(replace(&clients, &target, json!(1), "{}").unwrap_err().to_string().contains("delete it"));
            assert!(create_index(&clients, &target, "", false, None).unwrap_err().to_string().starts_with("the keys must be"));
            let unnamed = Target { collection: " ".into(), ..target.clone() };
            assert!(create_collection(&clients, &unnamed).is_err());
        });
    }

    #[test]
    fn a_document_shows_as_mongosh_text_with_its_id_kept_for_the_way_back() {
        let shown = view(&json!({ "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" }, "n": { "$numberLong": "7" } }));
        assert_eq!(shown.id, Some(json!({ "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" })));
        assert_eq!(shown.text, "{\n  _id: ObjectId(\"65f0a1b2c3d4e5f6a7b8c9d0\"),\n  n: NumberLong(7)\n}");
        assert_eq!(view(&json!({ "total": 3 })).id, None);
    }
}
