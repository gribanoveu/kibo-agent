//! The Kafka page's scenarios (`docs/54-kafka.md`): a saved connection checked,
//! the topic list with its search, one topic opened. The calls are
//! `infra::kafka_client`; the store is `infra::kafka_store`. What is decided
//! here is how much one answer carries and in what order.

use crate::domain::kafka::{page, Connection, ConnectionView, KafkaError, ProduceSpec, Produced, ReadPage, ReadSpec, TopicDetail, TopicList, TopicView};
use crate::infra::kafka_client::{self, KafkaClients};
use crate::infra::kafka_store;

/// How many topics one page carries. The page turns them; the search narrows
/// what the pages are of.
pub const PAGE_SIZE: usize = 100;

/// The saved connections, as the window lists them.
pub fn connections() -> Result<Vec<ConnectionView>, KafkaError> {
    kafka_store::list()
}

/// Adds or replaces a connection, and lets go of the pool it had: the settings
/// may be another cluster's.
pub fn save(clients: &KafkaClients, connection: Connection) -> Result<Vec<ConnectionView>, KafkaError> {
    let id = connection.id.clone();
    kafka_store::put(connection)?;
    clients.forget(&id);
    kafka_store::list()
}

pub fn delete(clients: &KafkaClients, id: &str) -> Result<Vec<ConnectionView>, KafkaError> {
    kafka_store::delete(id)?;
    clients.forget(id);
    kafka_store::list()
}

/// What the broker answers about itself, so a saved connection can be checked
/// without opening the page.
pub fn check(clients: &KafkaClients, id: &str) -> Result<String, KafkaError> {
    let connection = kafka_store::get(id)?;
    kafka_client::cluster(clients, &connection)
}

/// Topic names, searched by part of a name, one page at a time — with their
/// watermarks, and how many matched in all.
pub fn topics(clients: &KafkaClients, id: &str, name_part: Option<&str>, wanted: usize, refresh: bool) -> Result<TopicList, KafkaError> {
    let connection = kafka_store::get(id)?;
    if refresh {
        // Asked for by hand: the shape kept from two minutes ago is not the
        // answer wanted.
        clients.forget_shapes();
    }
    let names = kafka_client::names(clients, &connection)?;
    let (names, total) = page(names, name_part, wanted, PAGE_SIZE);
    let mut topics = kafka_client::topics(clients, &connection, &names)?;
    // A name the broker listed and then would not describe is gone, not a gap.
    topics.retain(|topic| names.iter().any(|name| name == &topic.name));
    Ok(TopicList { topics, total })
}

/// One topic opened: its partitions with watermarks and the configs a reader
/// cares about.
pub fn topic(clients: &KafkaClients, id: &str, name: &str) -> Result<TopicDetail, KafkaError> {
    let connection = kafka_store::get(id)?;
    let names = [name.to_string()];
    let mut described = kafka_client::topics(clients, &connection, &names)?;
    let topic: TopicView = described.pop().ok_or_else(|| KafkaError::Broker(format!("the cluster has no topic \"{name}\" any more")))?;
    let configs = kafka_client::configs(clients, &connection, name)?;
    Ok(TopicDetail { topic, configs })
}

/// Messages of a topic, read without a group: `spec` says which partitions,
/// from where and how many, and its ceilings are applied in the domain.
pub fn messages(clients: &KafkaClients, id: &str, spec: ReadSpec) -> Result<ReadPage, KafkaError> {
    let connection = kafka_store::get(id)?;
    kafka_client::read(clients, &connection, &spec)
}

/// Sends one record. A connection marked read-only refuses here, before the
/// broker is asked: the flag is the connection's own promise, not the page's.
pub fn send(clients: &KafkaClients, id: &str, spec: ProduceSpec) -> Result<Produced, KafkaError> {
    let connection = kafka_store::get(id)?;
    connection.producing()?;
    kafka_client::produce(clients, &connection, &spec.checked()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_of_topics_is_a_hundred() {
        // What the page turns and what the tool shows agree on one size.
        assert_eq!(PAGE_SIZE, 100);
    }
}
