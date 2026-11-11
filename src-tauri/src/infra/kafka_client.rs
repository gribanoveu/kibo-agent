//! The Kafka page's one way to a broker (`docs/54-kafka.md`): krafka's admin
//! API, every call already off the main thread (`commands::blocking`). The
//! only module that opens a connection to one — `data_policy::NETWORK_ALLOWED`
//! names it, and `krafka::` anywhere else fails that test.
//!
//! It carries no rules of its own: what a page holds, how many topics, in what
//! order — `services::kafka`. Here are the calls and their translation into
//! `domain::kafka` types.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use krafka::admin::{ConfigResource, OffsetSpec, TopicPartition};
use krafka::auth::{AuthConfig, TlsConfig};
use krafka::Kafka;

use crate::domain::kafka::{
    ConfigView, Connection, KafkaError, Message, PartitionView, ReadPage, ReadSpec, SaslMechanism, TopicView, MAX_READ_COUNT, MAX_READ_PARTITIONS,
};

/// How long a broker may take to be found: a wrong host says so in seconds.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// What one request may take once it is found.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How many partitions one page asks watermarks for. Two answers per partition
/// is two round trips' worth of work; past this the topics that did not fit are
/// left out rather than asked for one by one.
const MAX_PARTITIONS: usize = 4_000;

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
    ///
    /// Built outside the lock: a handshake is seconds, and every other command
    /// — including saving or deleting a connection — would wait behind it.
    /// Two commands that race here both connect; the first one in is kept and
    /// the other is dropped with its sockets.
    pub fn kafka(&self, connection: &Connection) -> Result<Arc<Kafka>, KafkaError> {
        let settings = fingerprint(connection);
        if let Some(kafka) = self.remembered(&connection.id, settings) {
            return Ok(kafka);
        }
        let kafka = Arc::new(connect(connection)?);
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((made, kept)) = clients.get(&connection.id) {
            if *made == settings {
                return Ok(kept.clone());
            }
        }
        clients.insert(connection.id.clone(), (settings, kafka.clone()));
        Ok(kafka)
    }

    fn remembered(&self, id: &str, settings: u64) -> Option<Arc<Kafka>> {
        let clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        clients.get(id).filter(|(made, _)| *made == settings).map(|(_, kafka)| kafka.clone())
    }

    /// A connection deleted: its pool is let go, and a call holding it finishes
    /// before its sockets close.
    pub fn forget(&self, id: &str) {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
    }

    /// Every pool let go: the feature is off, or its data was deleted. Each one
    /// holds a SASL password and open sockets to the user's cluster, so they go
    /// with the connections rather than at the end of the session.
    pub fn forget_all(&self) {
        self.clients.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

/// What the pool is keyed on: everything that changes where it connects or who
/// it is. The password only as a hash, and under a seed that is random per
/// process — a stable 64-bit print of a password next to a brokers list is an
/// offline guess-checker.
fn fingerprint(connection: &Connection) -> u64 {
    let mut hasher = salt().build_hasher();
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

fn salt() -> &'static std::collections::hash_map::RandomState {
    static SALT: OnceLock<std::collections::hash_map::RandomState> = OnceLock::new();
    SALT.get_or_init(std::collections::hash_map::RandomState::new)
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
    let described = within("the cluster", admin.describe_cluster(Default::default()))?;
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
    within("the topics", admin.list_topics(Default::default()))
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
    let described = within("the topics", admin.describe_topics(names, Default::default()))?;

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

    // Only as many partitions as one page carries: the topics that did not fit
    // are left out rather than asked for one partition at a time.
    let mut budget = MAX_PARTITIONS;
    let mut fitting: Vec<String> = Vec::new();
    for topic in &topics {
        if topic.partitions.len() <= budget {
            budget -= topic.partitions.len();
            fitting.push(topic.name.clone());
        }
    }
    topics.retain(|topic| fitting.contains(&topic.name));

    // The watermarks come in a map keyed by partition, so the beginning and the
    // end of the log are two answers, not one.
    let mut missing: HashSet<(String, i32)> = HashSet::new();
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
        let offsets = within("the watermarks", admin.list_offsets(specs, Default::default()))?;
        for topic in &mut topics {
            for partition in &mut topic.partitions {
                let at = TopicPartition::new(topic.name.clone(), partition.partition);
                let offset = offsets.get(&at).and_then(|listed| listed.as_ref().ok()).map(|listed| listed.offset);
                if !applied(partition, end, offset) {
                    missing.insert((topic.name.clone(), partition.partition));
                }
            }
        }
    }
    // A partition whose watermarks the broker would not give would show as an
    // empty one — a count of zero on a full partition is a lie, so the topic
    // that has one is left out instead.
    topics.retain(|topic| !topic.partitions.iter().any(|partition| missing.contains(&(topic.name.clone(), partition.partition))));
    Ok(topics)
}

/// The configs the page shows of one topic, by name.
pub fn configs(clients: &KafkaClients, connection: &Connection, topic: &str) -> Result<Vec<ConfigView>, KafkaError> {
    let kafka = clients.kafka(connection)?;
    let admin = kafka.admin();
    let resources = [ConfigResource::topic(topic)];
    let described = within("the topic's configs", admin.describe_configs(resources, Default::default()))?;
    let mut configs: Vec<ConfigView> = described
        .into_values()
        .filter_map(|entries| entries.ok())
        .flatten()
        .filter(|entry| SHOWN_CONFIGS.contains(&entry.name.as_str()))
        .map(|entry| ConfigView { name: entry.name, value: entry.value.unwrap_or_default() })
        .collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(configs)
}

/// Reads up to `count` messages of a topic's partitions, **without a group**:
/// `assign`, not `subscribe`, so the read leaves nothing behind in the cluster
/// and no member appears in a group the user never made.
///
/// `from` is where to start; `None` reads the last `count`, from as far back as
/// the watermarks say the log still has them.
pub fn read(clients: &KafkaClients, connection: &Connection, spec: &ReadSpec) -> Result<ReadPage, KafkaError> {
    let spec = spec.clone().clamped();
    let kafka = clients.kafka(connection)?;
    let admin = kafka.admin();
    let topic = spec.topic.clone();

    let mut partitions = spec.partitions.clone();
    if partitions.is_empty() {
        let described = within("the partitions", admin.describe_topics(&[topic.clone()], Default::default()))?;
        partitions = described
            .into_values()
            .filter_map(|described| described.ok())
            .flat_map(|described| described.partitions.into_iter().map(|partition| partition.partition))
            .collect();
        partitions.sort();
        partitions.truncate(MAX_READ_PARTITIONS);
    }
    if partitions.is_empty() {
        return Ok(ReadPage { messages: Vec::new(), partitions, more: false });
    }

    // Where each partition starts: the offset asked for, or far enough back to
    // hold `count` records — the watermarks are two answers, keyed by partition.
    let mut starts: Vec<(i32, i64)> = Vec::new();
    match spec.from {
        Some(from) => starts.extend(partitions.iter().map(|partition| (*partition, from))),
        None => {
            let specs: Vec<(TopicPartition, OffsetSpec)> = partitions.iter().map(|p| (TopicPartition::new(topic.clone(), *p), OffsetSpec::Earliest)).collect();
            let earliest = within("the watermarks", admin.list_offsets(specs, Default::default()))?;
            let specs: Vec<(TopicPartition, OffsetSpec)> = partitions.iter().map(|p| (TopicPartition::new(topic.clone(), *p), OffsetSpec::Latest)).collect();
            let latest = within("the watermarks", admin.list_offsets(specs, Default::default()))?;
            for partition in &partitions {
                let at = TopicPartition::new(topic.clone(), *partition);
                let start = earliest.get(&at).and_then(|o| o.as_ref().ok()).map(|o| o.offset).unwrap_or(0);
                let end = latest.get(&at).and_then(|o| o.as_ref().ok()).map(|o| o.offset).unwrap_or(start);
                starts.push((*partition, start.max(end - spec.count as i64)));
            }
        }
    }

    let count = spec.count.clamp(1, MAX_READ_COUNT);
    // Every failure of the consumer is one sentence: the broker's own text is
    // the useful half of it.
    let broke = |e: krafka::error::KrafkaError| KafkaError::Broker(format!("the messages: {e}"));
    let messages = within("the messages", async {
        let consumer = kafka.consumer_without_group().build().await.map_err(broke)?;
        consumer.assign(&topic, partitions.clone()).await.map_err(broke)?;
        for (partition, start) in &starts {
            consumer.seek(&topic, *partition, *start).await.map_err(broke)?;
        }
        let mut messages: Vec<Message> = Vec::new();
        let mut quiet = 0;
        let mut spent = 0u32;
        while messages.len() < count && quiet < 2 && spent < 16 {
            spent += 1;
            let batch = consumer.poll(Duration::from_millis(500)).await.map_err(broke)?;
            if batch.is_empty() {
                quiet += 1;
                continue;
            }
            quiet = 0;
            for record in batch {
                messages.push(Message {
                    partition: record.partition,
                    offset: record.offset,
                    timestamp: record.timestamp,
                    key: record.key.map(|key| key.to_vec()),
                    value: record.value.map(|value| value.to_vec()),
                    headers: record.headers.into_iter().map(|(name, value)| (name, value.map(|value| value.to_vec()))).collect(),
                });
            }
        }
        Ok::<Vec<Message>, KafkaError>(messages)
    })?;

    // The count may cut the answer: the page continues from the last offset it
    // was given, so saying so is all it needs.
    let more = messages.len() > count;
    let mut messages = messages;
    messages.truncate(count);
    messages.sort_by_key(|message| (message.timestamp, message.partition, message.offset));
    Ok(ReadPage { messages: messages.iter().map(Message::view).collect(), partitions, more })
}

/// One end of one partition's log, applied. `false` when the broker gave no
/// offset, which the caller reports rather than writing a zero that looks like
/// an empty partition.
fn applied(partition: &mut PartitionView, end: bool, offset: Option<i64>) -> bool {
    match (end, offset) {
        (true, Some(offset)) => {
            partition.latest = offset;
            true
        }
        (false, Some(offset)) => {
            partition.earliest = offset;
            true
        }
        _ => false,
    }
}

/// Every failure of the client becomes one sentence naming what was asked:
/// the broker's own error text is the useful half of it.
fn failed<T>(what: &str, result: Result<T, impl std::fmt::Display>) -> Result<T, KafkaError> {
    result.map_err(|e| KafkaError::Broker(format!("{what}: {e}")))
}

/// One call with a deadline of its own: the client's request timeout is a
/// socket one, and an answer that never comes must not hold a page — nor a
/// command that others wait on.
fn within<T, E: std::fmt::Display>(what: &str, future: impl Future<Output = Result<T, E>>) -> Result<T, KafkaError> {
    match tauri::async_runtime::block_on(async { tokio::time::timeout(REQUEST_TIMEOUT, future).await }) {
        Ok(answer) => failed(what, answer),
        Err(_) => Err(KafkaError::Broker(format!("{what}: no answer in {}s", REQUEST_TIMEOUT.as_secs()))),
    }
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

    /// The real broker of K-0, where one answers:
    /// `docker run -d --name kibo-kafka -p 9092:9092 docker.io/apache/kafka:4.4.0`,
    /// a topic `orders` on 3 partitions with three messages in it. Not ignored:
    /// it runs everywhere, and on a machine without the broker it says so and
    /// passes — the rules it checks have their own tests above.
    #[test]
    fn a_real_broker_is_read_when_one_answers() {
        let clients = KafkaClients::default();
        let connection = connection("localhost:9092");
        let Ok(line) = cluster(&clients, &connection) else {
            eprintln!("no broker on localhost:9092 — the live half of kafka_client is skipped");
            return;
        };
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

        assert!(topics(&clients, &connection, &["no-such-topic-x".to_string()]).unwrap().is_empty());

        // Messages, without a group: the three records K-0 wrote, with their keys.
        let page = read(&clients, &connection, &ReadSpec { topic: "orders".into(), partitions: vec![], from: None, count: 20 }).unwrap();
        assert_eq!(page.partitions.len(), 3, "{page:?}");
        assert_eq!(page.messages.len(), 3, "{page:?}");
        assert!(page.messages.iter().all(|message| message.key.is_some()), "the K-0 records carry keys");
        assert!(page.messages.iter().all(|message| message.size > 0 && !message.value.text().is_empty()));
    }

    /// The two ends of a log are two answers: a swap of them would leave every
    /// page counting zero or seventeen billion records.
    #[test]
    fn each_end_of_a_partition_lands_where_it_belongs() {
        let mut partition = PartitionView { partition: 0, leader: None, replicas: vec![], in_sync: vec![], earliest: 0, latest: 0 };
        assert!(applied(&mut partition, false, Some(90)));
        assert!(applied(&mut partition, true, Some(100)));
        assert_eq!((partition.earliest, partition.latest), (90, 100));
        assert!(!applied(&mut partition, true, None), "no answer is not an offset of zero");
        assert!(!applied(&mut partition, false, None));
        assert_eq!((partition.earliest, partition.latest), (90, 100), "and it writes nothing");
    }

    /// What the connection's two switches mean for the handshake: none is
    /// plaintext, each on its own is TLS or SASL, both is SCRAM over TLS.
    #[test]
    fn the_security_of_a_connection_follows_its_switches() {
        assert!(security(&connection("h:9092")).is_none(), "plaintext");

        let mut tls = connection("h:9092");
        tls.tls = true;
        let built = security(&tls).unwrap();
        assert_eq!(format!("{:?}", built.security_protocol()), "Ssl");
        assert!(built.sasl_mechanism().is_none());

        let mut login = connection("h:9092");
        login.sasl = Some(Sasl { mechanism: SaslMechanism::ScramSha512, username: "u".into(), password: "p".into() });
        let built = security(&login).unwrap();
        assert_eq!(format!("{:?}", built.security_protocol()), "SaslPlaintext");
        assert!(format!("{:?}", built.sasl_mechanism()).contains("ScramSha512"));

        let mut both = login.clone();
        both.tls = true;
        let built = security(&both).unwrap();
        assert_eq!(format!("{:?}", built.security_protocol()), "SaslSsl");
    }
}
