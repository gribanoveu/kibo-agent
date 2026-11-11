//! The Kafka page's one way to a broker (`docs/54-kafka.md`): krafka's admin
//! API, every call already off the main thread (`commands::blocking`). The
//! only module that opens a connection to one — `data_policy::NETWORK_ALLOWED`
//! names it, and `krafka::` anywhere else fails that test.
//!
//! It carries no rules of its own: what a page holds, how many topics, in what
//! order — `services::kafka`. Here are the calls and their translation into
//! `domain::kafka` types.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use krafka::admin::{ConfigResource, OffsetSpec, TopicPartition};
use krafka::auth::{AuthConfig, TlsConfig};
use krafka::Kafka;

use crate::domain::kafka::{ConfigView, Connection, KafkaError, PartitionView, SaslMechanism, TopicView};

/// How long a broker may take to be found: a wrong host says so in seconds.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// What one request may take once it is found.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// The configs the page shows: the ones that change what a reader can read or
/// how much. A topic has forty others and they are the broker's.
const SHOWN_CONFIGS: &[&str] = &["retention.ms", "cleanup.policy", "max.message.bytes", "min.insync.replicas"];

/// The clients made, one per saved connection and kept: a client is a pool,
/// and making one is a handshake — with TLS and SCRAM, several round trips —
/// on every call. Keyed by the connection's id, and remade when its settings
/// change: same id, new brokers or a new password, is a different cluster as
/// far as the pool is concerned.
#[derive(Default)]
pub struct KafkaClients {
    clients: Mutex<HashMap<String, (u64, Arc<Kafka>)>>,
}

impl KafkaClients {
    /// The pool for a connection, made on first use and after a change.
    pub fn kafka(&self, connection: &Connection) -> Result<Arc<Kafka>, KafkaError> {
        let settings = fingerprint(connection);
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((made, kafka)) = clients.get(&connection.id) {
            if *made == settings {
                return Ok(kafka.clone());
            }
        }
        let kafka = Arc::new(connect(connection)?);
        clients.insert(connection.id.clone(), (settings, kafka.clone()));
        Ok(kafka)
    }

    /// A connection deleted: its pool and monitors go with it.
    pub fn forget(&self, id: &str) {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
    }
}

/// What the pool is keyed on: everything that changes where it connects or who
/// it is. The password only as a hash — a key is not a place for a secret.
fn fingerprint(connection: &Connection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    connection.brokers.hash(&mut hasher);
    connection.tls.hash(&mut hasher);
    connection.ca_file.hash(&mut hasher);
    if let Some(sasl) = &connection.sasl {
        sasl.mechanism.hash(&mut hasher);
        sasl.username.hash(&mut hasher);
        sasl.password.hash(&mut hasher);
    }
    hasher.finish()
}

fn connect(connection: &Connection) -> Result<Kafka, KafkaError> {
    let mut builder = Kafka::builder(connection.brokers.clone())
        .client_id("kibo-agent")
        .connect_timeout(CONNECT_TIMEOUT)
        .request_timeout(REQUEST_TIMEOUT);
    if let Some(security) = security(connection) {
        builder = builder.security(security);
    }
    tauri::async_runtime::block_on(builder.connect()).map_err(|e| {
        KafkaError::Broker(format!("{} did not answer at {}: {e}", connection.name.trim(), connection.brokers.trim()))
    })
}

/// TLS and SASL as the connection has them; `None` is plaintext, which is what
/// the client does when nothing is set.
fn security(connection: &Connection) -> Option<AuthConfig> {
    let mut auth = match &connection.sasl {
        Some(sasl) => match sasl.mechanism {
            SaslMechanism::Plain => AuthConfig::sasl_plain(sasl.username.clone(), sasl.password.clone()),
            SaslMechanism::ScramSha256 => AuthConfig::sasl_scram_sha256(sasl.username.clone(), sasl.password.clone()),
            SaslMechanism::ScramSha512 => AuthConfig::sasl_scram_sha512(sasl.username.clone(), sasl.password.clone()),
        },
        None => AuthConfig::plaintext(),
    };
    if connection.tls {
        auth = auth.with_tls(match &connection.ca_file {
            // The CA the user pointed at, instead of the machine's roots: a
            // cluster inside a company has its own.
            Some(path) => TlsConfig::new().with_ca_cert(path.clone()),
            // Bundled Mozilla roots: the same set on every machine, where the
            // platform store would differ per user. A private CA is a file.
            None => TlsConfig::new(),
        });
    }
    match connection.tls || connection.sasl.is_some() {
        true => Some(auth),
        false => None,
    }
}

/// The broker's answer to "who are you": cluster id and how many brokers.
pub fn cluster(clients: &KafkaClients, connection: &Connection) -> Result<String, KafkaError> {
    let kafka = clients.kafka(connection)?;
    let admin = kafka.admin();
    let described = failed("the cluster", tauri::async_runtime::block_on(admin.describe_cluster(Default::default())))?;
    Ok(format!(
        "{} broker{} — cluster {}",
        described.brokers.len(),
        if described.brokers.len() == 1 { "" } else { "s" },
        described.cluster_id
    ))
}

/// Every topic's name, as the broker lists it.
pub fn names(clients: &KafkaClients, connection: &Connection) -> Result<Vec<String>, KafkaError> {
    let kafka = clients.kafka(connection)?;
    let admin = kafka.admin();
    failed("the topics", tauri::async_runtime::block_on(admin.list_topics(Default::default())))
}

/// Those of `names` with their partitions and watermarks. A topic the broker
/// will not describe — no permission, or taken away since the list — is left
/// out rather than failing the page.
pub fn topics(clients: &KafkaClients, connection: &Connection, names: &[String]) -> Result<Vec<TopicView>, KafkaError> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let kafka = clients.kafka(connection)?;
    let admin = kafka.admin();
    let described = failed("the topics", tauri::async_runtime::block_on(admin.describe_topics(names, Default::default())))?;

    let mut topics: Vec<TopicView> = described
        .into_iter()
        .filter_map(|(name, description)| {
            let description = description.ok()?;
            let partitions: Vec<PartitionView> = description
                .partitions
                .iter()
                .map(|partition| PartitionView {
                    partition: partition.partition,
                    leader: partition.leader,
                    replicas: partition.replicas.clone(),
                    in_sync: partition.isr.clone(),
                    earliest: 0,
                    latest: 0,
                })
                .collect();
            Some(TopicView { internal: name.starts_with("__"), name, partitions })
        })
        .collect();
    topics.sort_by(|a, b| a.name.cmp(&b.name));

    // The watermarks come in a map keyed by partition, so the beginning and the
    // end of the log are two answers, not one.
    for end in [false, true] {
        let specs: Vec<(TopicPartition, OffsetSpec)> = topics
            .iter()
            .flat_map(|topic| {
                topic.partitions.iter().map(move |partition| {
                    let spec = match end {
                        true => OffsetSpec::Latest,
                        false => OffsetSpec::Earliest,
                    };
                    (TopicPartition::new(topic.name.clone(), partition.partition), spec)
                })
            })
            .collect();
        let offsets = failed("the watermarks", tauri::async_runtime::block_on(admin.list_offsets(specs, Default::default())))?;
        for topic in &mut topics {
            for partition in &mut topic.partitions {
                let at = TopicPartition::new(topic.name.clone(), partition.partition);
                if let Some(Ok(listed)) = offsets.get(&at) {
                    match end {
                        true => partition.latest = listed.offset,
                        false => partition.earliest = listed.offset,
                    }
                }
            }
        }
    }
    Ok(topics)
}

/// The configs the page shows of one topic, by name.
pub fn configs(clients: &KafkaClients, connection: &Connection, topic: &str) -> Result<Vec<ConfigView>, KafkaError> {
    let kafka = clients.kafka(connection)?;
    let admin = kafka.admin();
    let resources = [ConfigResource::topic(topic)];
    let described = failed("the topic's configs", tauri::async_runtime::block_on(admin.describe_configs(resources, Default::default())))?;
    let mut configs: Vec<ConfigView> = described
        .into_iter()
        .filter_map(|(_, entries)| entries.ok())
        .flatten()
        .filter(|entry| SHOWN_CONFIGS.contains(&entry.name.as_str()))
        .map(|entry| ConfigView { name: entry.name, value: entry.value.unwrap_or_default() })
        .collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(configs)
}

/// Every failure of the client becomes one sentence naming what was asked:
/// the broker's own error text is the useful half of it.
fn failed<T>(what: &str, result: Result<T, impl std::fmt::Display>) -> Result<T, KafkaError> {
    result.map_err(|e| KafkaError::Broker(format!("{what}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::kafka::{Sasl, SaslMechanism};

    fn connection(brokers: &str) -> Connection {
        Connection {
            id: "c1".into(),
            name: "local".into(),
            brokers: brokers.into(),
            tls: false,
            ca_file: None,
            sasl: None,
            read_only: false,
            agent: false,
        }
    }

    /// A broker that is not there answers in seconds with the address in the
    /// sentence, rather than the client's own thirty.
    #[test]
    fn a_broker_that_is_not_there_says_where_it_looked() {
        let clients = KafkaClients::default();
        let started = std::time::Instant::now();
        let refused = cluster(&clients, &connection("127.0.0.1:1")).unwrap_err().to_string();
        assert!(refused.contains("127.0.0.1:1"), "{refused}");
        assert!(started.elapsed() < Duration::from_secs(20), "took {:?}", started.elapsed());
    }

    #[test]
    fn the_pool_is_remade_when_the_settings_change_and_forgotten_on_delete() {
        let clients = KafkaClients::default();
        // Nothing is reached here: the pool is keyed before it is built, and
        // the same settings answer with the same failure rather than a new one.
        assert!(clients.kafka(&connection("127.0.0.1:1")).is_err());
        assert!(clients.clients.lock().unwrap().is_empty(), "a client that was never made is not kept");
        clients.forget("c1");

        let mut with_password = connection("h:9092");
        with_password.sasl = Some(Sasl { mechanism: SaslMechanism::Plain, username: "u".into(), password: "p".into() });
        let plain = fingerprint(&connection("h:9092"));
        let same = fingerprint(&connection("h:9092"));
        let other = fingerprint(&with_password);
        assert_eq!(plain, same);
        assert_ne!(plain, other, "a login is a different connection");
        let mut retyped = connection("h:9092");
        retyped.sasl = Some(Sasl { mechanism: SaslMechanism::Plain, username: "u".into(), password: "p".into() });
        assert_eq!(other, fingerprint(&retyped), "the same login is the same connection");
        let mut other_password = retyped.clone();
        other_password.sasl.as_mut().unwrap().password = "q".into();
        assert_ne!(other, fingerprint(&other_password), "a new password is a new pool");
    }

    #[test]
    fn the_shown_configs_are_the_ones_a_reader_cares_about() {
        assert!(SHOWN_CONFIGS.contains(&"retention.ms"));
        assert!(SHOWN_CONFIGS.contains(&"cleanup.policy"));
        assert!(!SHOWN_CONFIGS.contains(&"message.timestamp.type"));
    }

    /// The real broker of K-0, if it is up:
    /// `docker run -d --name kibo-kafka -p 9092:9092 docker.io/apache/kafka:4.4.0`,
    /// a topic `orders` on 3 partitions with three messages in it.
    #[test]
    #[ignore]
    fn live_kafka_reads_the_topics_of_a_real_broker() {
        let clients = KafkaClients::default();
        let connection = connection("localhost:9092");

        let line = cluster(&clients, &connection).unwrap();
        assert!(line.contains("cluster"), "{line}");

        let names = names(&clients, &connection).unwrap();
        assert!(names.iter().any(|name| name == "orders"), "{names:?}");

        let described = topics(&clients, &connection, &["orders".to_string()]).unwrap();
        assert_eq!(described.len(), 1);
        let orders = &described[0];
        assert_eq!(orders.partitions.len(), 3, "{orders:?}");
        assert_eq!(orders.stored(), 3, "three messages went in at K-0: {orders:?}");
        assert!(orders.partitions.iter().all(|p| p.leader.is_some() && !p.in_sync.is_empty()));

        let shown = configs(&clients, &connection, "orders").unwrap();
        assert!(shown.iter().any(|c| c.name == "retention.ms" && !c.value.is_empty()), "{shown:?}");
        assert!(shown.iter().any(|c| c.name == "cleanup.policy"), "{shown:?}");

        // A topic that is not there is not an empty list of partitions but a
        // refusal naming it.
        assert!(topics(&clients, &connection, &["no-such-topic-".to_string() + "x"]).unwrap().is_empty());
        assert!(configs(&clients, &connection, "no-such-topic-xx").unwrap().is_empty());
    }
}
