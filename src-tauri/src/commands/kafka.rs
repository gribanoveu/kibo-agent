//! The Kafka page (`docs/54-kafka.md`), through `services::kafka`. No channel
//! of its own yet: only the page changes what it shows, and each change
//! answers with what it changed — the list of connections after a save, the
//! topics after a search.

use std::sync::Arc;

use tauri::State;

use crate::domain::kafka::{Connection, ConnectionView, ProduceSpec, Produced, ReadPage, ReadSpec, TopicDetail, TopicList};
use crate::infra::kafka_client::KafkaClients;
use crate::services::kafka;

use super::blocking;

type Clients<'a> = State<'a, Arc<KafkaClients>>;

#[tauri::command]
pub async fn kafka_connections() -> Result<Vec<ConnectionView>, String> {
    blocking(kafka::connections).await
}

#[tauri::command]
pub async fn kafka_connection_save(connection: Connection, clients: Clients<'_>) -> Result<Vec<ConnectionView>, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::save(&clients, connection)).await
}

#[tauri::command]
pub async fn kafka_connection_delete(id: String, clients: Clients<'_>) -> Result<Vec<ConnectionView>, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::delete(&clients, &id)).await
}

/// What the broker says about itself.
#[tauri::command]
pub async fn kafka_connection_check(id: String, clients: Clients<'_>) -> Result<String, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::check(&clients, &id)).await
}

/// A page of topics — `name` searches part of a name, any case; `page` counts
/// from zero, a hundred topics a page.
#[tauri::command]
pub async fn kafka_topics(connection: String, name: Option<String>, page: Option<usize>, refresh: Option<bool>, clients: Clients<'_>) -> Result<TopicList, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::topics(&clients, &connection, name.as_deref(), page.unwrap_or(0), refresh.unwrap_or(false))).await
}

/// A page of a topic's messages — `spec` names the partitions, the offset and
/// how many; its ceilings are the domain's.
#[tauri::command]
pub async fn kafka_messages(connection: String, spec: ReadSpec, clients: Clients<'_>) -> Result<ReadPage, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::messages(&clients, &connection, spec)).await
}

/// Sends one record: `spec` carries the topic, the key, the headers and the
/// text value. Answers the partition and the offset it landed at.
#[tauri::command]
pub async fn kafka_produce(connection: String, spec: ProduceSpec, clients: Clients<'_>) -> Result<Produced, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::send(&clients, &connection, spec)).await
}

#[tauri::command]
pub async fn kafka_topic(connection: String, topic: String, clients: Clients<'_>) -> Result<TopicDetail, String> {
    let clients = clients.inner().clone();
    blocking(move || kafka::topic(&clients, &connection, &topic)).await
}
