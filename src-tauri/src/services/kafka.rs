//! The Kafka page's scenarios (`docs/54-kafka.md`): a saved connection checked,
//! the topic list with its search, one topic opened. The calls are
//! `infra::kafka_client`; the store is `infra::kafka_store`. What is decided
//! here is how much one answer carries and in what order.

use crate::domain::kafka::{page, Connection, GroupList, LiveSink, LiveSpec, ConnectionView, KafkaError, ProduceSpec, Produced, ReadPage, ReadSpec, TopicDetail, TopicList, TopicView};
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

/// The live reads this app has started: a stop flag each. A read ends when its
/// flag is set, when the connection is gone, or when the broker stops answering
/// — the thread is a `spawn_blocking` task, so the window never waits for it.
#[derive(Default)]
pub struct KafkaLive {
    running: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>>>,
}

impl KafkaLive {
    /// Starts one and answers its id. Everything that can fail — the connection,
    /// the topic, the broker going quiet — is said on the sink, in a batch of
    /// its own, so the window has one place to watch and one way to learn.
    pub fn start(&self, clients: std::sync::Arc<KafkaClients>, connection: Connection, spec: LiveSpec, sink: LiveSink) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(id.clone(), stop.clone());
        let ending = self.running.clone();
        let mine = id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            // A failure is said on the sink by `live` itself; the entry goes
            // either way, so the map is what runs, not what ran.
            if let Err(failure) = kafka_client::live(&clients, &connection, &spec, stop, sink) {
                eprintln!("kafka live {mine} ended: {failure}");
            }
            ending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&mine);
        });
        id
    }

    /// Stops one. The flag is what ends it; the entry goes so the map does not
    /// grow with every toggle.
    pub fn stop(&self, id: &str) {
        if let Some(stop) = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(id) {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Everything goes quiet: a connection deleted, the feature turned off.
    pub fn stop_all(&self) {
        for (_, stop) in self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// How many are running, and the flag of one — what a test can see without
    /// a broker.
    #[cfg(test)]
    fn running(&self) -> usize {
        self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    #[cfg(test)]
    fn flag(&self, id: &str) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(id).cloned()
    }
}

/// The cluster's consumer groups, with their lag.
pub fn groups(clients: &KafkaClients, id: &str) -> Result<GroupList, KafkaError> {
    let connection = kafka_store::get(id)?;
    kafka_client::groups(clients, &connection)
}

/// A live read started from the window: the connection is read here, so the
/// thread behind the read gets a whole one and the command only an id.
pub fn live_start(live: &KafkaLive, clients: &std::sync::Arc<KafkaClients>, id: &str, spec: LiveSpec, sink: LiveSink) -> Result<String, KafkaError> {
    let connection = kafka_store::get(id)?;
    Ok(live.start(clients.clone(), connection, spec, sink))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_read_is_kept_until_it_is_stopped() {
        let live = KafkaLive::default();
        assert_eq!(live.running(), 0);
        // A connection that never connects: the thread ends at once, but the
        // entry is the handle's, not the thread's.
        let clients = std::sync::Arc::new(KafkaClients::default());
        let connection = || Connection { id: "c1".into(), name: "prod".into(), brokers: "127.0.0.1:1".into(), tls: false, ca_file: None, sasl: None, read_only: true, agent: false };
        let spec = || LiveSpec { topic: "orders".into(), partitions: vec![0], from: None };
        let id = live.start(clients, connection(), spec(), std::sync::Arc::new(|_| {}));
        assert_eq!(live.running(), 1);
        let flag = live.flag(&id).expect("a running read has a flag");
        assert!(!flag.load(std::sync::atomic::Ordering::Relaxed), "a fresh read runs");

        live.stop(&id);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed), "the flag is what ends the thread");
        assert_eq!(live.running(), 0, "a stopped read is not kept");
        live.stop(&id);
        assert_eq!(live.running(), 0, "stopping twice changes nothing");
    }

    #[test]
    fn everything_goes_quiet_when_the_feature_does() {
        let live = KafkaLive::default();
        let clients = std::sync::Arc::new(KafkaClients::default());
        let connection = || Connection { id: "c1".into(), name: "prod".into(), brokers: "127.0.0.1:1".into(), tls: false, ca_file: None, sasl: None, read_only: true, agent: false };
        let spec = || LiveSpec { topic: "orders".into(), partitions: vec![0], from: None };
        let first = live.start(clients.clone(), connection(), spec(), std::sync::Arc::new(|_| {}));
        let second = live.start(clients, connection(), spec(), std::sync::Arc::new(|_| {}));
        assert_eq!(live.running(), 2);

        // Held outside the map: the entry is a handle, the flag is what ends
        // the thread, and a test that looks only at the map cannot tell.
        let flags: Vec<_> = [&first, &second].iter().filter_map(|id| live.flag(id)).collect();
        assert_eq!(flags.len(), 2);
        live.stop_all();
        assert_eq!(live.running(), 0);
        assert!(flags.iter().all(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)), "every read was told to stop");
    }

    #[test]
    fn a_page_of_topics_is_a_hundred() {
        // What the page turns and what the tool shows agree on one size.
        assert_eq!(PAGE_SIZE, 100);
    }
}
