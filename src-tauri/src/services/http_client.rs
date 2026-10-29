//! The HTTP page's use cases over its two stores (`docs/45-http-client.md`):
//! a Postman file imported into whichever it belongs to, and saved responses
//! whose request is gone dropped.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::domain::http_client::HttpError;
use crate::domain::postman::{read_import, Imported};
use crate::infra::{http_collections, http_env_store};

/// What an Import brought in, for the page to open it.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ImportedAs {
    Collection { id: String, name: String },
    Environment { id: String, name: String },
    Globals { count: usize },
}

pub fn import(text: &str) -> Result<ImportedAs, HttpError> {
    match read_import(text)? {
        Imported::Collection(collection) => Ok(ImportedAs::Collection { id: http_collections::add(&collection)?, name: collection.name() }),
        Imported::Environment(environment) => {
            let done = ImportedAs::Environment { id: environment.id.clone(), name: environment.name.clone() };
            http_env_store::add(environment)?;
            Ok(done)
        }
        Imported::Globals(globals) => {
            let count = globals.len();
            http_env_store::set_globals(globals)?;
            Ok(ImportedAs::Globals { count })
        }
    }
}

/// Run as the page opens: a collection imported again with other ids leaves
/// the old ids' responses behind.
pub fn prune_responses() -> Result<(), HttpError> {
    let known: HashMap<String, Option<HashSet<String>>> =
        http_collections::all()?.into_iter().map(|(id, collection)| (id, collection.ok().map(|c| c.request_ids().into_iter().collect()))).collect();
    http_collections::prune_responses(&known)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::{Outcome, SavedResponse};
    use crate::testing::with_app_dir;

    const PROBE: &str = include_str!("../infra/testdata/postman/probe.postman_collection.json");

    #[test]
    fn each_kind_of_file_lands_in_its_store_and_a_secret_is_sealed() {
        with_app_dir("http-import", || {
            let ImportedAs::Collection { id, name } = import(PROBE).unwrap() else { panic!("a collection") };
            assert_eq!(name, "wire_kibo_test");
            assert_eq!(http_collections::get(&id).unwrap().request_ids().len(), 10, "with the ids it was given");

            let env = r#"{"name": "Stage", "values": [{"key": "token", "value": "pm-hunter2", "type": "secret", "enabled": true}], "_postman_variable_scope": "environment"}"#;
            let ImportedAs::Environment { id, name } = import(env).unwrap() else { panic!("an environment") };
            assert_eq!(name, "Stage");
            assert_eq!(http_env_store::get(&id).unwrap().variables[0].value, "pm-hunter2");
            assert_eq!(http_env_store::list().unwrap()[0].variables[0].value, "•••");
            let raw = std::fs::read(http_collections::dir().unwrap().join("environments.enc")).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("pm-hunter2"));

            let globals = r#"{"values": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}], "_postman_variable_scope": "globals"}"#;
            assert_eq!(import(globals).unwrap(), ImportedAs::Globals { count: 2 });
            assert!(matches!(import("{\"openapi\": \"3.0.0\"}"), Err(HttpError::NotACollection(_))));
        });
    }

    #[test]
    fn a_response_whose_request_is_gone_is_pruned() {
        with_app_dir("http-prune", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let request = http_collections::get(&id).unwrap().request_ids()[0].clone();
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new() };
            http_collections::save_response(&id, &request, &saved).unwrap();
            http_collections::save_response(&id, "removed-request", &saved).unwrap();
            http_collections::save_response("removed-collection", "r", &saved).unwrap();
            let ImportedAs::Collection { id: broken, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            http_collections::save_response(&broken, "r", &saved).unwrap();
            let file = http_collections::dir().unwrap().join("collections").join(format!("{broken}.postman_collection.json"));
            std::fs::write(file, "{ damaged").unwrap();
            prune_responses().unwrap();
            assert!(http_collections::last_response(&id, &request).unwrap().is_some());
            assert_eq!(http_collections::last_response(&id, "removed-request").unwrap(), None);
            assert_eq!(http_collections::last_response("removed-collection", "r").unwrap(), None);
            assert!(http_collections::last_response(&broken, "r").unwrap().is_some(), "a file that does not read keeps its responses");
        });
    }
}
