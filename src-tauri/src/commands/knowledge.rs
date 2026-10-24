//! The Knowledge page (`docs/40-knowledge-base.md`, decision 9), through
//! `services::knowledge`. No channel of its own: the page re-reads when a
//! `knowledgeWrite` ends on `chat:turn-event`, and each change here answers
//! with the base as it is after it.

use crate::domain::knowledge::{Base, Domain};
use crate::services::knowledge::{self, KnowledgePageError};

use super::blocking;

/// The base after `change`.
fn then_base(change: impl FnOnce() -> Result<(), KnowledgePageError>) -> Result<Base, KnowledgePageError> {
    change()?;
    Ok(knowledge::base()?)
}

#[tauri::command]
pub async fn knowledge_base() -> Result<Base, String> {
    blocking(knowledge::base).await
}

/// A document's file as kept, frontmatter first.
#[tauri::command]
pub async fn knowledge_document(id: String) -> Result<String, String> {
    blocking(move || knowledge::document(&id)).await
}

#[tauri::command]
pub async fn knowledge_move(id: String, domain: String) -> Result<Base, String> {
    blocking(move || then_base(|| knowledge::move_document(&id, &domain))).await
}

#[tauri::command]
pub async fn knowledge_delete(id: String) -> Result<Base, String> {
    blocking(move || then_base(|| knowledge::delete_document(&id))).await
}

#[tauri::command]
pub async fn knowledge_delete_repo(repo: String) -> Result<Base, String> {
    blocking(move || then_base(|| knowledge::delete_repo(&repo))).await
}

#[tauri::command]
pub async fn knowledge_domain_edit(domain: Domain) -> Result<Base, String> {
    blocking(move || then_base(|| knowledge::edit_domain(&domain))).await
}

/// Every document of `from` moved into `into`, and `from` gone.
#[tauri::command]
pub async fn knowledge_domain_merge(from: String, into: String) -> Result<Base, String> {
    blocking(move || then_base(|| knowledge::merge_domains(&from, &into))).await
}
