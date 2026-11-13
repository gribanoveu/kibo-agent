//! The Kafka page (`docs/54-kafka.md`): what a connection is, what crosses to
//! the window about it, and what a message's bytes are shown as.
//!
//! The message view is the part with rules in it. Kafka carries bytes and
//! nothing else, so the view guesses, in this order: JSON if the bytes parse, a
//! Confluent-framed protobuf, a bare wire message, printable UTF-8 text, hex.
//! The protobuf dump is by field number and needs no schema — it is what
//! `protoc --decode_raw` prints — and that is the whole reason this page shows
//! protobuf without a Schema Registry. A length-delimited field is a nested
//! message only if it parses as one; otherwise it is text (when it reads as
//! UTF-8) or escaped bytes.
//!
//! No client types here: `infra::kafka_client` turns krafka's records into
//! [`Message`], and a record to send back into krafka's.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The most text one message's view carries, escaped text included. Longer
/// values are cut and say so; a value past [`MAX_DECODE`] is measured and cut
/// the same way, without being parsed at all.
const TEXT_LIMIT: usize = 64 * 1024;
/// What one value may be before nothing is parsed: a 5 MiB blob is not what
/// anyone is looking at, and parsing it is the slowest thing on the page.
/// Public so a reader of its own — the agent's tool — keeps the same limit.
pub const MAX_DECODE: usize = 1024 * 1024;

/// How many messages are followed below the first one: the level past it is
/// left as bytes rather than parsed.
const MAX_DEPTH: usize = 64;

/// The largest field number protobuf allows, 2^29 - 1. `protoc` refuses more,
/// and so does this: a dump that took any number would read ordinary binary as
/// a message far more often.
const MAX_FIELD: u32 = 536_870_911;

// ---------------------------------------------------------------- connection

/// A saved connection, as it is sealed: the password is inside [`Sasl`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    /// `bootstrap.servers` — `host:port`, several separated by commas.
    pub brokers: String,
    #[serde(default)]
    pub tls: bool,
    /// A PEM file with the CA to trust, when the broker has its own.
    #[serde(default)]
    pub ca_file: Option<String>,
    #[serde(default)]
    pub sasl: Option<Sasl>,
    /// Nothing is sent through it: the page's form and `kafkaProduce` refuse.
    /// A file sealed before the flag existed reads as off.
    #[serde(default)]
    pub read_only: bool,
    /// Open to the agent's tools, by name; off until the user ticks it.
    #[serde(default)]
    pub agent: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sasl {
    pub mechanism: SaslMechanism,
    pub username: String,
    pub password: String,
}

/// The password is wiped wherever a login is let go — the store's list, the
/// copy a client was made from, an edit's replaced one — not only where
/// someone remembered to.
impl Drop for Sasl {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.password);
    }
}

/// Never the password: a `{:?}` of a connection is what a log line or a dump
/// of a tool's arguments would print.
impl std::fmt::Debug for Sasl {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Sasl")
            .field("mechanism", &self.mechanism)
            .field("username", &self.username)
            .field("password", &"•••")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SaslMechanism {
    Plain,
    ScramSha256,
    ScramSha512,
}

/// A connection as the window sees it: where it goes and who it is, never the
/// password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub id: String,
    pub name: String,
    pub brokers: String,
    pub tls: bool,
    /// The CA it trusts, when it has its own: a path, not a secret, so the
    /// form can prefill it.
    pub ca_file: Option<String>,
    pub login: Option<LoginView>,
    /// A password is saved: an edit with the field left empty keeps it.
    pub has_password: bool,
    pub read_only: bool,
    /// Open to the agent's tools. `agent`, not `openToAgent`: the name the
    /// sealed file uses, as `domain::mongo` has it.
    pub agent: bool,
}

/// Who the connection is, in the pieces the form edits: one
/// `SCRAM-SHA-512 as reports` line could not be read back, since a username may
/// itself contain " as ".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginView {
    pub mechanism: SaslMechanism,
    pub username: String,
}

impl From<&Connection> for ConnectionView {
    fn from(connection: &Connection) -> Self {
        Self {
            id: connection.id.clone(),
            name: connection.name.clone(),
            brokers: connection.brokers.clone(),
            tls: connection.tls,
            ca_file: connection.ca_file.clone(),
            login: connection.sasl.as_ref().map(|sasl| LoginView { mechanism: sasl.mechanism, username: sasl.username.clone() }),
            has_password: connection.sasl.as_ref().is_some_and(|sasl| !sasl.password.is_empty()),
            read_only: connection.read_only,
            agent: connection.agent,
        }
    }
}

impl Connection {
    /// Refused before a client is built, so a read-only cluster's only answer
    /// to a send is this sentence rather than a broker's error.
    pub fn producing(&self) -> Result<(), KafkaError> {
        // The name as it stands, trimmed: whether it is empty is the form's
        // question, not this one's.
        if self.read_only {
            return Err(KafkaError::ReadOnly(self.name.trim().to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KafkaError {
    #[error("no saved connection \"{0}\" — it was deleted")]
    NoConnection(String),
    #[error("\"{0}\" is read-only — nothing is sent through it")]
    ReadOnly(String),
    #[error("{0}")]
    Store(String),
    #[error("{0}")]
    Broker(String),
    /// The agent's tool, with no saved connection ticked "Open to the agent".
    #[error("no Kafka connection is open to the agent — the user opens one on the Kafka page: edit it and turn on \"Open to the agent\"")]
    NoneOpen,
    #[error("no connection named \"{name}\" is open to the agent — open to it: {open}")]
    NotOpen { name: String, open: String },
    #[error("{count} connections open to the agent are named \"{name}\" — ask the user to rename one on the Kafka page")]
    SameName { name: String, count: usize },
}

// -------------------------------------------------------------------- topics

/// A topic as the page lists and opens it, with every partition's watermarks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicView {
    pub name: String,
    pub internal: bool,
    pub partitions: Vec<PartitionView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartitionView {
    pub partition: i32,
    pub leader: Option<i32>,
    pub replicas: Vec<i32>,
    pub in_sync: Vec<i32>,
    /// The first offset still on the broker.
    pub earliest: i64,
    /// One past the last offset written.
    pub latest: i64,
}

impl PartitionView {
    /// How many records are on the broker here. Segments removed by retention
    /// are gone from both ends, so the difference is what is readable.
    pub fn stored(&self) -> i64 {
        (self.latest - self.earliest).max(0)
    }
}

impl TopicView {
    pub fn stored(&self) -> i64 {
        self.partitions.iter().map(PartitionView::stored).sum()
    }

    /// What the tool and the page's list show on one line.
    pub fn describe(&self) -> String {
        format!(
            "{} — {} partition{}, {} record{}",
            self.name,
            self.partitions.len(),
            if self.partitions.len() == 1 { "" } else { "s" },
            self.stored(),
            if self.stored() == 1 { "" } else { "s" }
        )
    }
}

/// Topics as one answer carries them: one page, and how many matched in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicList {
    pub topics: Vec<TopicView>,
    pub total: usize,
}

/// One topic opened: its partitions and the configs a reader cares about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDetail {
    pub topic: TopicView,
    pub configs: Vec<ConfigView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub name: String,
    pub value: String,
}

/// Whether something read at `at` is still worth using. A broker's shape of a
/// topic — its partitions, replicas, in-sync copies — moves on the scale of
/// minutes; its watermarks move with every record. The two are kept apart, and
/// this is the rule for the first.
pub fn fresh(at: std::time::Instant, now: std::time::Instant, ttl: std::time::Duration) -> bool {
    now.saturating_duration_since(at) < ttl
}

/// Whether a topic's name answers a search: part of it, any case; nothing
/// asked is everything.
pub fn matches(name: &str, part: Option<&str>) -> bool {
    match part.map(str::trim).filter(|part| !part.is_empty()) {
        Some(part) => name.to_lowercase().contains(&part.to_lowercase()),
        None => true,
    }
}

/// One page of topics: the search applied, sorted, the slice from `page *
/// size`, and how many names matched in all — so the page can say "101–200 of
/// 240" and nothing has to guess. Pure, because none of these rules needs a
/// broker to check.
pub fn page(mut names: Vec<String>, part: Option<&str>, page: usize, size: usize) -> (Vec<String>, usize) {
    names.retain(|name| matches(name, part));
    names.sort();
    let total = names.len();
    let from = page.saturating_mul(size).min(total);
    let to = (from + size).min(total);
    (names[from..to].to_vec(), total)
}

/// How many partitions one read may span, and how many messages one answer
/// carries: a page is a page.
pub const MAX_READ_PARTITIONS: usize = 20;
pub const MAX_READ_COUNT: usize = 100;

/// What to read: a topic's partitions, from which offset, how many.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadSpec {
    pub topic: String,
    /// Empty is every partition of the topic, up to [`MAX_READ_PARTITIONS`].
    #[serde(default)]
    pub partitions: Vec<i32>,
    /// The offset to read from; `None` is "the last `count`".
    #[serde(default)]
    pub from: Option<i64>,
    #[serde(default)]
    pub count: usize,
}

impl ReadSpec {
    /// The spec with its two ceilings applied. Clamping here rather than in
    /// the client keeps the promise one function, whoever asks.
    pub fn clamped(mut self) -> Self {
        self.partitions.truncate(MAX_READ_PARTITIONS);
        self.count = self.count.clamp(1, MAX_READ_COUNT);
        self
    }
}

/// Where a read of `count` messages starts in one partition: the offset asked
/// for, kept inside the log, or — for "the last `count`" — far enough back to
/// hold them. Clamping the asked-for offset is what turns `-1` or an offset
/// past the end into a readable answer instead of a broker's complaint.
pub fn starting(from: Option<i64>, earliest: i64, latest: i64, count: usize) -> i64 {
    match from {
        Some(offset) => offset.clamp(earliest, latest),
        None => latest.saturating_sub(count as i64).max(earliest),
    }
}

/// How many messages one partition of a read may contribute: a fair share of
/// the count, so one busy partition cannot fill the answer alone. `poll`
/// returns records partition by partition, and a plain cut would be that
/// partition's alone.
pub fn quota(count: usize, partitions: usize) -> usize {
    count.div_ceil(partitions.max(1)).max(1)
}

/// What one send may carry, the whole record — key, headers and value — since
/// a broker refuses more by default (`max.message.bytes`).
pub const MAX_SENT: usize = 1_048_576;

/// Headers one record may carry: a record is small, and a thousand of them is
/// not a message anyone reads.
pub const MAX_HEADERS: usize = 64;

/// A record to send. The value is text, UTF-8: nothing here knows a schema, so
/// a protobuf payload cannot be assembled from the window (decision 2).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProduceSpec {
    pub topic: String,
    /// `None` lets the producer's own partitioner choose.
    #[serde(default)]
    pub partition: Option<i32>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub value: String,
}

impl std::fmt::Debug for ProduceSpec {
    /// Never the words themselves: a value or a key in a log is someone's data.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProduceSpec")
            .field("topic", &self.topic)
            .field("partition", &self.partition)
            .field("key", &self.key.as_ref().map(|_| "•••"))
            .field("headers", &self.headers.iter().map(|(name, _)| name).collect::<Vec<_>>())
            .field("value", &format!("{} bytes", self.value.len()))
            .finish()
    }
}

impl ProduceSpec {
    /// The spec as the broker may see it. An empty value is a record with
    /// nothing in it, which Kafka allows and compaction keeps — it is not the
    /// same as a tombstone, and this cannot write one.
    pub fn checked(mut self) -> Result<Self, KafkaError> {
        // The topic as it goes to the broker: a stray space is a name it has
        // never heard of, not a name to look for.
        self.topic = self.topic.trim().to_string();
        if self.topic.is_empty() {
            return Err(KafkaError::Broker("a message needs a topic".into()));
        }
        if self.partition.is_some_and(|partition| partition < 0) {
            return Err(KafkaError::Broker("a partition is zero or more".into()));
        }
        if self.headers.len() > MAX_HEADERS {
            return Err(KafkaError::Broker(format!("a record carries at most {MAX_HEADERS} headers")));
        }
        if self.headers.iter().any(|(name, _)| name.trim().is_empty()) {
            return Err(KafkaError::Broker("a header needs a name".into()));
        }
        // Everything the record carries, not its value alone: a broker's
        // `max.message.bytes` counts the record.
        let carried = self.value.len()
            + self.key.as_ref().map_or(0, String::len)
            + self.headers.iter().map(|(name, value)| name.len() + value.len()).sum::<usize>();
        if carried > MAX_SENT {
            return Err(KafkaError::Broker(format!("the record is over {MAX_SENT} bytes")));
        }
        Ok(self)
    }
}

/// One tick of a live read: what arrived since the last one. `more` is the
/// count still waiting for the next tick — records are never dropped, only
/// carried — and `failed` is the end of the read, said where it can be seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBatch {
    pub messages: Vec<MessageView>,
    pub more: usize,
    pub failed: Option<String>,
}

/// What one tick takes: the messages waiting, in order, up to
/// [`LIVE_PER_TICK`], and the rest to carry into the next tick. A rule of the
/// window's own, so it can be checked without a broker.
pub fn live_tick(mut waiting: Vec<Message>, per_tick: usize) -> (Vec<Message>, Vec<Message>) {
    waiting.sort_by_key(|message| (message.partition, message.offset));
    if waiting.len() <= per_tick {
        return (waiting, Vec::new());
    }
    let rest = waiting.split_off(per_tick);
    (waiting, rest)
}

/// Where a live read reports: the window's channel, adapted in `commands/`.
/// `false` is "the window is gone" — the read ends there, since nothing is
/// left to report to and a reloaded window has no way to stop it.
pub type LiveSink = std::sync::Arc<dyn Fn(LiveBatch) -> bool + Send + Sync>;

/// Where a live read starts: the partitions asked for (`None` is every one of
/// them, up to [`MAX_READ_PARTITIONS`]) from their end, or from an offset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSpec {
    pub topic: String,
    #[serde(default)]
    pub partitions: Vec<i32>,
    /// `None` is "from now on": the log's end when the read starts.
    #[serde(default)]
    pub from: Option<i64>,
}

/// How many messages one tick carries, and how often a tick is.
pub const LIVE_PER_TICK: usize = 200;
pub const LIVE_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// Where a sent record landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Produced {
    pub partition: i32,
    pub offset: i64,
}

/// One answer: the messages, the partitions they came from, and whether the
/// count ran out before the log did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadPage {
    pub messages: Vec<MessageView>,
    pub partitions: Vec<i32>,
    pub more: bool,
}

/// One partition's share of a group's lag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LagView {
    pub group: String,
    pub topic: String,
    pub partition: i32,
    /// The group's committed offset; `None` when it never committed here.
    pub committed: Option<i64>,
    pub latest: i64,
    /// `latest - committed`, never negative. Kafka's own lag is the same
    /// number; a partition without a commit has no lag to speak of, and shows
    /// as no data rather than as zero.
    pub lag: Option<i64>,
}

/// Whether a listed group is one this page shows: a consumer group, or one
/// with no protocol of its own — an offset-only group looks like that, and its
/// lag is real. A `connect` or a `transactional` group has no lag to speak of.
pub fn is_consumer_group(protocol_type: &str) -> bool {
    protocol_type.is_empty() || protocol_type == "consumer"
}

/// A cluster's groups as one answer carries them: what was read, how many
/// there are in all, and how many brokers would not answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupList {
    pub groups: Vec<GroupView>,
    /// Groups the cluster listed, before the answer's ceiling.
    pub total: usize,
    /// Brokers that did not answer: their groups are missing from this list,
    /// not absent from the cluster.
    pub unread_brokers: usize,
}

/// A consumer group as one answer carries it: its state, how many members hold
/// it, and the lag of every partition it committed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub name: String,
    /// `Stable`, `Empty`, `PreparingRebalance`, … as the broker words it.
    pub state: String,
    pub members: usize,
    /// Whether the lag was read for this group: past the page's ceiling it is
    /// not, and `total_lag: None` then means "not asked", not "no commits".
    pub lags_read: bool,
    pub lags: Vec<LagView>,
    /// The lag over the partitions that have one; `None` when none does — a
    /// group that never committed has no lag to speak of, and a zero here would
    /// read as "caught up".
    pub total_lag: Option<i64>,
}

impl GroupView {
    pub fn new(name: String, state: String, members: usize, lags_read: bool, lags: Vec<LagView>) -> Self {
        let total_lag = lags.iter().filter_map(|lag| lag.lag).reduce(|sum, lag| sum + lag);
        Self { name, state, members, lags_read, lags, total_lag }
    }
}

impl LagView {
    pub fn new(group: &str, topic: &str, partition: i32, committed: Option<i64>, latest: i64) -> Self {
        Self {
            group: group.to_string(),
            topic: topic.to_string(),
            partition,
            committed,
            latest,
            lag: committed.map(|committed| (latest - committed).max(0)),
        }
    }
}

// ------------------------------------------------------------------ messages

/// One record, as the client hands it over: bytes, nothing interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub partition: i32,
    pub offset: i64,
    /// Milliseconds since the epoch, as the broker has it.
    pub timestamp: i64,
    pub key: Option<Vec<u8>>,
    pub value: Option<Vec<u8>>,
    pub headers: Vec<(String, Option<Vec<u8>>)>,
}

/// A record as the window draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub partition: i32,
    pub offset: i64,
    pub timestamp: i64,
    pub key: Option<ValueView>,
    pub value: ValueView,
    /// The value's size on the wire, before any view was chosen.
    pub size: usize,
    pub headers: Vec<HeaderView>,
}

/// A header: a name, and its bytes — `None` for one written with no value,
/// which is not the same as an empty one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderView {
    pub name: String,
    pub value: Option<String>,
}

/// What a message's bytes turned out to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ValueView {
    Empty,
    Json { text: String, truncated: bool },
    /// A raw protobuf dump: `1: "…"`, `5: 30`, nested in braces. `schema_id`
    /// is set when the payload carried a Confluent envelope — the id is shown,
    /// the schema is not fetched.
    Protobuf { text: String, schema_id: Option<u32>, truncated: bool },
    Text { text: String, truncated: bool },
    Hex { text: String, size: usize, truncated: bool },
}

impl ValueView {
    /// The text a person reads and the model is given, for either view.
    pub fn text(&self) -> String {
        match self {
            ValueView::Empty => String::new(),
            ValueView::Json { text, .. } | ValueView::Protobuf { text, .. } | ValueView::Text { text, .. } | ValueView::Hex { text, .. } => text.clone(),
        }
    }
}

impl Message {
    pub fn view(&self) -> MessageView {
        MessageView {
            partition: self.partition,
            offset: self.offset,
            timestamp: self.timestamp,
            key: self.key.as_deref().map(view),
            value: self.value.as_deref().map(view).unwrap_or(ValueView::Empty),
            size: self.value.as_ref().map_or(0, Vec::len),
            headers: self.headers.iter().map(|(name, value)| HeaderView { name: name.clone(), value: value.as_deref().map(dump_string) }).collect(),
        }
    }
}

/// The guess this page makes about a value: JSON, a Confluent-framed protobuf,
/// text, a bare protobuf, hex.
pub fn view(bytes: &[u8]) -> ValueView {
    if bytes.is_empty() {
        return ValueView::Empty;
    }
    // Past this size nothing is parsed: a 5 MiB blob in a `<pre>` is not what
    // anyone is looking at, and the parse would be the slowest thing on the page.
    if bytes.len() > MAX_DECODE {
        return hex(bytes);
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
        let (text, truncated) = cut(&serde_json::to_string_pretty(&value).unwrap_or_default());
        return ValueView::Json { text, truncated };
    }
    if let Some((schema_id, body)) = confluent(bytes) {
        if let Ok(message) = ProtoMessage::decode(body) {
            let (text, truncated) = cut(&message.render());
            return ValueView::Protobuf { text, schema_id: Some(schema_id), truncated };
        }
    }
    // A wire message before text, as the plan's decision 2 has it: a protobuf
    // value whose first field is one long string — a tag of `0x0a`, a length of
    // 32 or more, then text — reads as printable UTF-8 and would be shown as
    // text with its own tag bytes in front. Plain text that happens to parse is
    // rarer, and the page's view switch covers the guess either way.
    if let Ok(message) = ProtoMessage::decode(bytes) {
        let (text, truncated) = cut(&message.render());
        return ValueView::Protobuf { text, schema_id: None, truncated };
    }
    if let Some(text) = printable(bytes) {
        let (text, truncated) = cut(&text);
        return ValueView::Text { text, truncated };
    }
    hex(bytes)
}

/// The last resort: the bytes, escaped, cut to what one view carries. Bytes
/// dropped before `dump_string` are truncated too — the dump of what is left
/// fits, and would otherwise claim to be the whole value.
fn hex(bytes: &[u8]) -> ValueView {
    let shown = bytes.len().min(TEXT_LIMIT);
    let (text, cut_short) = cut(&dump_string(&bytes[..shown]));
    ValueView::Hex { text, size: bytes.len(), truncated: cut_short || shown < bytes.len() }
}

/// A Confluent-framed payload: a zero byte and the schema id, then the message.
/// The id is all that is read of it — the schema itself stays where it is.
fn confluent(bytes: &[u8]) -> Option<(u32, &[u8])> {
    match bytes {
        [0, a, b, c, d, rest @ ..] => Some((u32::from_be_bytes([*a, *b, *c, *d]), rest)),
        _ => None,
    }
}

fn cut(text: &str) -> (String, bool) {
    if text.len() <= TEXT_LIMIT {
        return (text.to_string(), false);
    }
    (format!("{}…", &text[..floor_char(text, TEXT_LIMIT)]), true)
}

/// The last boundary at or below `at`, so a cut never lands inside a character.
fn floor_char(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The bytes as text, when they are UTF-8 and nothing but control characters
/// (a newline, a tab) spoil it. A protobuf message almost never is: its first
/// byte is a tag, and tags are control bytes.
fn printable(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let clean = !text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'));
    clean.then(|| text.to_string())
}

/// A value as the dump writes it between quotes: printable text stays as it
/// is, a quote or a backslash is escaped, and a control byte — a newline, a
/// NUL — or a byte that is not UTF-8 becomes `\xNN`. One field per line holds
/// only if a newline cannot get through it.
fn dump_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for character in chunk.valid().chars() {
            match character {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                awkward if awkward.is_control() => {
                    let mut buffer = [0u8; 4];
                    for byte in awkward.encode_utf8(&mut buffer).as_bytes() {
                        out.push_str(&format!("\\x{byte:02x}"));
                    }
                }
                plain => out.push(plain),
            }
        }
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out
}

// -------------------------------------------------------- the raw protobuf dump

/// A protobuf message read without its schema: field numbers and wire values.
/// Private on purpose: what the window and the model get is [`ValueView`], and
/// a stage that needs the tree can widen this when it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtoMessage {
    fields: Vec<ProtoField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtoField {
    number: u32,
    value: ProtoValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProtoValue {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    /// A length-delimited value that parses as a message of its own.
    Message(Vec<ProtoField>),
    /// A length-delimited value that does not: text when it reads as UTF-8,
    /// otherwise bytes written as `\xNN`.
    Blob(Vec<u8>),
    /// A deprecated group: fields up to its end tag.
    Group(Vec<ProtoField>),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
enum ProtoError {
    #[error("the message ends inside a value at byte {at}")]
    Truncated { at: usize },
    #[error("a varint at byte {at} does not fit in 64 bits")]
    Overlong { at: usize },
    #[error("a tag at byte {at} names field 0, which does not exist")]
    ZeroField { at: usize },
    #[error("a tag at byte {at} names field {number}, and protobuf has none past {}", MAX_FIELD)]
    FieldTooLarge { at: usize, number: u64 },
    #[error("a tag at byte {at} has wire type {wire}, which no field uses")]
    BadWireType { at: usize, wire: u8 },
    #[error("a length at byte {at} asks for {len} bytes with {left} left")]
    TooLong { at: usize, len: usize, left: usize },
    #[error("nested deeper than {max} at byte {at}")]
    TooDeep { max: usize, at: usize },
    #[error("an end-group at byte {at} closes no group")]
    StrayEndGroup { at: usize },
    #[error("a group at byte {at} is closed by field {found}, not {expected}")]
    MismatchedGroup { at: usize, expected: u32, found: u32 },
    #[error("a group at byte {at} is never closed")]
    UnclosedGroup { at: usize },
}

impl ProtoMessage {
    fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let (fields, _) = decode_fields(bytes, 0, MAX_DEPTH, None)?;
        Ok(Self { fields })
    }

    /// The dump `protoc --decode_raw` prints, with the one difference this app
    /// wants: text stays text. `9: "Перевод средств"`, not `9: "\320\237…"`.
    fn render(&self) -> String {
        let mut out = String::new();
        render_fields(&self.fields, 0, &mut out);
        out
    }
}

/// The fields from `at` to the end of `bytes` — or, inside a group (`group`
/// is its field number and where it opened), to its end tag, answering where
/// that tag ended.
fn decode_fields(bytes: &[u8], at: usize, depth: usize, group: Option<(u32, usize)>) -> Result<(Vec<ProtoField>, usize), ProtoError> {
    if depth == 0 {
        // A group has no length that could become bytes, so one nested past the
        // limit fails the dump instead of turning into a blob as a message does.
        // Groups are deprecated and rare; a missing dump is better than a wrong
        // one.
        return Err(ProtoError::TooDeep { max: MAX_DEPTH, at });
    }
    let mut fields = Vec::new();
    let mut at = at;
    while at < bytes.len() {
        let tag_at = at;
        let tag = varint(bytes, &mut at)?;
        let number = field_number(tag, tag_at)?;
        let wire = (tag & 0x7) as u8;
        // An end tag closes the group it is inside; at message level it closes
        // none.
        if wire == 4 {
            return match group {
                Some((open, _)) if open == number => Ok((fields, at)),
                Some((open, _)) => Err(ProtoError::MismatchedGroup { at: tag_at, expected: open, found: number }),
                None => Err(ProtoError::StrayEndGroup { at: tag_at }),
            };
        }
        fields.push(decode_field_with_tag(bytes, &mut at, number, wire, depth, tag_at)?);
    }
    match group {
        Some((_, opened)) => Err(ProtoError::UnclosedGroup { at: opened }),
        None => Ok((fields, at)),
    }
}

/// A tag's field number, refused when protobuf has no such field: 0 does not
/// exist, and anything past 2^29 - 1 is not a message but ordinary binary.
fn field_number(tag: u64, at: usize) -> Result<u32, ProtoError> {
    let number = tag >> 3;
    match number {
        0 => Err(ProtoError::ZeroField { at }),
        number if number > u64::from(MAX_FIELD) => Err(ProtoError::FieldTooLarge { at, number }),
        number => Ok(number as u32),
    }
}

fn decode_field_with_tag(bytes: &[u8], at: &mut usize, number: u32, wire: u8, depth: usize, tag_at: usize) -> Result<ProtoField, ProtoError> {
    let value = match wire {
        0 => ProtoValue::Varint(varint(bytes, at)?),
        1 => ProtoValue::Fixed64(fixed(bytes, at, 8)?),
        5 => ProtoValue::Fixed32(fixed(bytes, at, 4)? as u32),
        2 => {
            let length_at = *at;
            let length = varint(bytes, at)? as usize;
            let left = bytes.len() - *at;
            if length > left {
                return Err(ProtoError::TooLong { at: length_at, len: length, left });
            }
            let body = &bytes[*at..*at + length];
            *at += length;
            match decode_fields(body, 0, depth - 1, None) {
                Ok((fields, _)) if !fields.is_empty() => ProtoValue::Message(fields),
                _ => ProtoValue::Blob(body.to_vec()),
            }
        }
        3 => {
            let (fields, end) = decode_fields(bytes, *at, depth - 1, Some((number, tag_at)))?;
            *at = end;
            ProtoValue::Group(fields)
        }
        wire => return Err(ProtoError::BadWireType { at: tag_at, wire }),
    };
    Ok(ProtoField { number, value })
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, ProtoError> {
    let mut value: u64 = 0;
    // Nine bytes of payload, each free to say another follows.
    for index in 0..9 {
        let start = *at;
        let Some(&byte) = bytes.get(*at) else { return Err(ProtoError::Truncated { at: start }) };
        *at += 1;
        value |= ((byte & 0x7f) as u64) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    // The tenth carries the one bit still free, and has to end the varint: 0
    // or 1, nothing else fits in 64 bits.
    let start = *at;
    let Some(&byte) = bytes.get(*at) else { return Err(ProtoError::Truncated { at: start }) };
    *at += 1;
    match byte {
        0 | 1 => Ok(value | ((byte as u64) << 63)),
        _ => Err(ProtoError::Overlong { at: start }),
    }
}

fn fixed(bytes: &[u8], at: &mut usize, width: usize) -> Result<u64, ProtoError> {
    let start = *at;
    let Some(slice) = bytes.get(start..start + width) else { return Err(ProtoError::Truncated { at: start }) };
    *at += width;
    Ok(slice.iter().rev().fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
}

fn render_fields(fields: &[ProtoField], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for field in fields {
        out.push_str(&format!("{indent}{}: ", field.number));
        match &field.value {
            ProtoValue::Varint(value) => out.push_str(&format!("{value}\n")),
            ProtoValue::Fixed64(value) => out.push_str(&format!("{value}\n")),
            ProtoValue::Fixed32(value) => out.push_str(&format!("{value}\n")),
            ProtoValue::Blob(bytes) => out.push_str(&format!("\"{}\"\n", dump_string(bytes))),
            ProtoValue::Message(fields) | ProtoValue::Group(fields) => {
                out.push_str("{\n");
                render_fields(fields, depth + 1, out);
                out.push_str(&format!("{indent}}}\n"));
            }
        }
    }
}

// --------------------------------------------------------------- the agent

/// What the agent's `kafka` tool reads, through the same services the page
/// uses (`docs/54-kafka.md`, K-9). A trait rather than the clients, so the
/// tool is tested without a broker — and so the tool layer keeps no socket.
///
/// Records cross as bytes — [`Message`], not [`MessageView`] — because the
/// tool chooses the view and redacts what it shows, as `mongo` redacts the
/// documents it reads.
pub trait KafkaServers: Send + Sync {
    /// The saved connections, without their passwords.
    fn connections(&self) -> Result<Vec<ConnectionView>, KafkaError>;
    /// One page of the topics whose name has `part` in it, any case; `None`
    /// asks for every topic. `total` is how many matched in all.
    fn topics(&self, connection: &str, part: Option<&str>, page: usize) -> Result<TopicList, KafkaError>;
    /// One topic, with its partitions' watermarks and the configs a reader cares about.
    fn topic(&self, connection: &str, name: &str) -> Result<TopicDetail, KafkaError>;
    /// A window of a topic's records, read without a group.
    fn messages(&self, connection: &str, spec: ReadSpec) -> Result<MessagePage, KafkaError>;
    /// A window looked through for one key — a scan, since Kafka indexes
    /// nothing by key.
    fn scan(&self, connection: &str, spec: ScanSpec) -> Result<ScanPage, KafkaError>;
}

/// A window of records as the tool reads them: bytes, and how many there were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub partitions: Vec<i32>,
    /// Records follow past the last one here.
    pub more: bool,
}

/// A look through a topic's records for one key. Kafka has no index by key: a
/// key is part of the record, and the broker hands out offsets only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSpec {
    pub topic: String,
    /// Empty is every partition of the topic, up to [`MAX_READ_PARTITIONS`].
    pub partitions: Vec<i32>,
    /// Where it starts looking; `None` is the oldest offset still stored.
    pub from: Option<i64>,
    /// The key looked for, as text: a record's key is bytes, and the model
    /// writes text.
    pub key: String,
}

/// What a key scan found, and what it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanPage {
    pub messages: Vec<Message>,
    pub partitions: Vec<i32>,
    /// Records looked at, found or not.
    pub scanned: usize,
    /// The scan stopped at a ceiling with the log unread — there may be more.
    pub more: bool,
}

/// What one key scan may cost, so that a topic of a million records is not
/// read on a model's guess: records looked at, bytes of values, and time per
/// partition up to [`SCAN_TIME_MOST`] in all — whichever comes first.
pub const MAX_SCAN: usize = 20_000;
pub const SCAN_BYTES: usize = 8 * 1024 * 1024;
pub const SCAN_TIME: std::time::Duration = std::time::Duration::from_secs(5);
/// A tool call the turn waits on: twenty partitions are not a hundred seconds.
pub const SCAN_TIME_MOST: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a scan of `partitions` may look.
pub fn scan_time(partitions: usize) -> std::time::Duration {
    (SCAN_TIME * partitions.max(1) as u32).min(SCAN_TIME_MOST)
}

/// Whether a scan has spent what it may.
pub fn scan_over(scanned: usize, bytes: usize, spent: std::time::Duration, partitions: usize) -> bool {
    scanned >= MAX_SCAN || bytes >= SCAN_BYTES || spent >= scan_time(partitions)
}

/// Whether a record's key is the one looked for. Compared as bytes: a key that
/// is not UTF-8 cannot be the text the model sent.
pub fn key_is(key: Option<&[u8]>, wanted: &str) -> bool {
    key == Some(wanted.as_bytes())
}

/// Whether a scan looked at its partitions to the end of the log: `next` is one
/// past the last record read on a partition, `latest` where that partition
/// ends. A partition left short of its end has something unread, and then
/// "nothing has that key" is a guess rather than a fact — the broker going
/// quiet for two polls is not the end of a log.
pub fn reached_end(next: &[(i32, i64)], latest: &[(i32, i64)]) -> bool {
    latest.iter().all(|(partition, end)| next.iter().any(|(read, at)| read == partition && at >= end))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scan stops at the first of its three ceilings, and the clock is per
    /// partition up to a ceiling of its own: twenty of them are thirty seconds.
    #[test]
    fn a_scan_stops_at_the_first_of_its_ceilings() {
        let s = |scanned: usize, bytes: usize, seconds: u64, partitions: usize| {
            scan_over(scanned, bytes, std::time::Duration::from_secs(seconds), partitions)
        };
        assert!(!s(MAX_SCAN - 1, 0, 0, 1), "a record short of the ceiling is still looking");
        assert!(s(MAX_SCAN, 0, 0, 1));
        assert!(!s(0, SCAN_BYTES - 1, 0, 1), "a byte short of the ceiling is still looking");
        assert!(s(0, SCAN_BYTES, 0, 1));
        assert!(!s(0, 0, 4, 1), "four seconds of one partition is not the five it may take");
        assert!(s(0, 0, 5, 1));
        assert!(!s(0, 0, 5, 2), "the time is each partition's own");
        assert!(s(0, 0, 10, 2));
        assert!(s(0, 0, 5, 0), "no partition still means one's worth");
        assert!(!s(0, 0, 29, 20));
        assert!(s(0, 0, 30, 20), "twenty partitions stop at the ceiling, not at a hundred seconds");
    }

    /// A key is bytes and the model writes text: only the very same bytes
    /// match, in the same case.
    #[test]
    fn a_key_is_the_same_bytes_or_nothing() {
        assert!(key_is(Some(b"user-1"), "user-1"));
        assert!(!key_is(Some(b"user-1"), "user-10"));
        assert!(!key_is(Some(b"user-1"), "User-1"), "a key is not a topic's name: case matters");
        assert!(!key_is(Some(&[0xff, 0x00]), "ÿ"), "bytes that are not UTF-8 are no text at all");
        assert!(!key_is(None, ""), "a record with no key is not the empty key");
    }

    /// "Everything stored" is said only when every partition was read to its
    /// own end: one record short, or a partition never looked at, leaves
    /// something the key could be in.
    #[test]
    fn a_scan_is_at_the_end_of_the_log_only_when_every_partition_is() {
        let next = [(0, 10), (1, 5)];
        assert!(reached_end(&next, &[(0, 10), (1, 5)]));
        assert!(reached_end(&next, &[(0, 9)]), "an empty partition is at its end as soon as it starts");
        assert!(!reached_end(&next, &[(0, 10), (1, 6)]), "a record short is not the end");
        assert!(!reached_end(&next, &[(0, 10), (2, 0)]), "a partition never looked at is not read");
        assert!(reached_end(&[], &[]), "a scan of nothing is over");
    }

    /// The bytes of a fixture, built rather than hand-written: a varint, a
    /// length-delimited field, and the fields of the example in the plan.
    fn enc_varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            match value {
                0 => {
                    out.push(byte);
                    return out;
                }
                _ => out.push(byte | 0x80),
            }
        }
    }

    fn enc_tag(field: u32, wire: u8) -> Vec<u8> {
        enc_varint((u64::from(field) << 3) | u64::from(wire))
    }

    fn field_varint(field: u32, value: u64) -> Vec<u8> {
        let mut out = enc_tag(field, 0);
        out.extend(enc_varint(value));
        out
    }

    fn field_bytes(field: u32, body: &[u8]) -> Vec<u8> {
        let mut out = enc_tag(field, 2);
        out.extend(enc_varint(body.len() as u64));
        out.extend_from_slice(body);
        out
    }

    fn field_str(field: u32, text: &str) -> Vec<u8> {
        field_bytes(field, text.as_bytes())
    }

    /// The message from the plan: a payment instruction, as it looks without
    /// its schema.
    fn example() -> Vec<u8> {
        let mut date = field_varint(1, 2026);
        date.extend(field_varint(2, 1));
        let mut money = field_varint(1, 810);
        money.extend(field_str(2, "RUR"));
        money.extend(field_varint(3, 100));
        money.extend(field_str(4, "€"));
        money.extend(field_str(5, "Российский рубль"));
        let mut amount = field_varint(1, 600_000);
        amount.extend(field_bytes(2, &money));

        let mut out = field_str(1, "1260112MOXK#DZ1000122");
        out.extend(field_str(2, "UDSNAH"));
        out.extend(field_bytes(3, &date));
        out.extend(field_bytes(4, &date));
        out.extend(field_varint(5, 30));
        out.extend(field_str(7, "Перевод собственных средств;09/01/2026"));
        out.extend(field_bytes(8, &amount));
        out
    }

    #[test]
    fn the_message_from_the_plan_renders_as_it_does_with_text_left_readable() {
        let message = ProtoMessage::decode(&example()).unwrap();
        assert_eq!(
            message.render(),
            "1: \"1260112MOXK#DZ1000122\"\n\
             2: \"UDSNAH\"\n\
             3: {\n  1: 2026\n  2: 1\n}\n\
             4: {\n  1: 2026\n  2: 1\n}\n\
             5: 30\n\
             7: \"Перевод собственных средств;09/01/2026\"\n\
             8: {\n  1: 600000\n  2: {\n    1: 810\n    2: \"RUR\"\n    3: 100\n    4: \"€\"\n    5: \"Российский рубль\"\n  }\n}\n"
        );
    }

    #[test]
    fn a_value_view_of_that_message_is_a_protobuf_dump() {
        match view(&example()) {
            ValueView::Protobuf { text, schema_id, truncated } => {
                assert!(text.starts_with("1: \"1260112MOXK#DZ1000122\""), "{text}");
                assert_eq!(schema_id, None);
                assert!(!truncated);
            }
            other => panic!("not a protobuf dump: {other:?}"),
        }
    }

    #[test]
    fn a_utf8_string_with_cyrillic_is_not_escaped() {
        assert_eq!(printable("Перевод".as_bytes()).as_deref(), Some("Перевод"));
        let mut bytes = enc_tag(2, 2);
        bytes.extend(enc_varint("Перевод".len() as u64));
        bytes.extend("Перевод".as_bytes());
        assert_eq!(ProtoMessage::decode(&bytes).unwrap().render(), "2: \"Перевод\"\n");
    }

    #[test]
    fn bytes_that_are_not_utf8_are_written_as_hex_escapes() {
        let mut bytes = enc_tag(2, 2);
        bytes.extend(enc_varint(2));
        bytes.extend([0xff, 0xfe]);
        assert_eq!(ProtoMessage::decode(&bytes).unwrap().render(), "2: \"\\xff\\xfe\"\n");
        assert_eq!(dump_string(&[b'a', 0xff]), "a\\xff");
    }

    #[test]
    fn a_value_that_is_utf8_but_not_printable_is_not_text() {
        assert_eq!(printable(&[0x08, 0x01]), None, "a control byte is not text");
        assert_eq!(printable(b"two\nlines\tok"), Some("two\nlines\tok".to_string()));
    }

    #[test]
    fn quotes_and_backslashes_in_text_are_escaped_once() {
        let bytes = field_str(1, "say \"hi\" \\ bye");
        assert_eq!(ProtoMessage::decode(&bytes).unwrap().render(), "1: \"say \\\"hi\\\" \\\\ bye\"\n");
    }

    #[test]
    fn json_is_shown_pretty_and_chosen_before_anything_else() {
        match view(br#"{"orderId":"A-1","amount":1500}"#) {
            ValueView::Json { text, truncated } => {
                assert!(text.contains("\n  \"amount\": 1500"), "{text}");
                assert!(!truncated);
            }
            other => panic!("not json: {other:?}"),
        }
    }

    #[test]
    fn plain_text_is_text_and_not_a_protobuf_message() {
        assert_eq!(view(b"hello"), ValueView::Text { text: "hello".into(), truncated: false });
    }

    #[test]
    fn an_empty_value_is_empty() {
        assert_eq!(view(b""), ValueView::Empty);
    }

    #[test]
    fn a_confluent_envelope_names_its_schema_and_the_dump_skips_it() {
        let mut bytes = vec![0, 0, 0, 0, 42];
        bytes.extend(field_varint(1, 7));
        match view(&bytes) {
            ValueView::Protobuf { text, schema_id, .. } => {
                assert_eq!(schema_id, Some(42));
                assert_eq!(text, "1: 7\n");
            }
            other => panic!("not protobuf: {other:?}"),
        }
        assert_eq!(confluent(&bytes).map(|(id, body)| (id, body.len())), Some((42, 2)));
        assert_eq!(confluent(&[0, 0, 0]), None, "too short to hold an id");
        assert_eq!(confluent(b"\x08\x01"), None, "no envelope without a zero byte");
    }

    #[test]
    fn binary_that_is_neither_protobuf_nor_text_is_hex_with_its_size() {
        // A tag of field 0: no protobuf message can start that way.
        let bytes = [0x00, 0xff, 0xff];
        match view(&bytes) {
            ValueView::Hex { text, size, truncated } => {
                assert_eq!(size, 3);
                assert_eq!(text, "\\x00\\xff\\xff");
                assert!(!truncated);
            }
            other => panic!("not hex: {other:?}"),
        }
    }

    #[test]
    fn a_zero_field_is_refused() {
        assert_eq!(ProtoMessage::decode(&[0x00, 0x01]).unwrap_err(), ProtoError::ZeroField { at: 0 });
        assert_eq!(ProtoMessage::decode(&enc_tag(0, 0)).unwrap_err(), ProtoError::ZeroField { at: 0 });
    }

    #[test]
    fn wire_types_six_and_seven_are_refused_with_their_offset() {
        assert_eq!(ProtoMessage::decode(&[0x0e]).unwrap_err(), ProtoError::BadWireType { at: 0, wire: 6 });
        assert_eq!(ProtoMessage::decode(&[0x0f]).unwrap_err(), ProtoError::BadWireType { at: 0, wire: 7 });
    }

    #[test]
    fn a_length_past_the_end_says_how_much_was_left() {
        let bytes = [0x0a, 0x05, b'a'];
        assert_eq!(ProtoMessage::decode(&bytes).unwrap_err(), ProtoError::TooLong { at: 1, len: 5, left: 1 });
    }

    #[test]
    fn a_varint_that_stops_early_names_the_offset_of_its_last_byte() {
        assert_eq!(ProtoMessage::decode(&[0x08, 0x80]).unwrap_err(), ProtoError::Truncated { at: 2 });
    }

    #[test]
    fn a_varint_of_ten_bytes_is_read_and_a_longer_one_is_refused() {
        let ten = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
        let mut bytes = enc_tag(1, 0);
        bytes.extend(ten);
        assert_eq!(ProtoMessage::decode(&bytes).unwrap().fields[0].value, ProtoValue::Varint(u64::MAX));
        let overlong = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        let mut bytes = enc_tag(1, 0);
        bytes.extend(overlong);
        assert!(matches!(ProtoMessage::decode(&bytes), Err(ProtoError::Overlong { .. })));
    }

    #[test]
    fn fixed_fields_are_read_little_endian() {
        let mut bytes = enc_tag(3, 1);
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(enc_tag(4, 5));
        bytes.extend(0xdead_beefu32.to_le_bytes());
        let message = ProtoMessage::decode(&bytes).unwrap();
        assert_eq!(message.fields[0].value, ProtoValue::Fixed64(1));
        assert_eq!(message.fields[1].value, ProtoValue::Fixed32(0xdead_beef));
    }

    #[test]
    fn a_group_opens_and_closes_and_a_stray_end_is_refused() {
        let mut bytes = enc_tag(9, 3);
        bytes.extend(field_varint(1, 5));
        bytes.extend(enc_tag(9, 4));
        let message = ProtoMessage::decode(&bytes).unwrap();
        assert_eq!(message.fields[0].value, ProtoValue::Group(vec![ProtoField { number: 1, value: ProtoValue::Varint(5) }]));

        let stray = [0x0c];
        assert_eq!(ProtoMessage::decode(&stray).unwrap_err(), ProtoError::StrayEndGroup { at: 0 });

        let mismatched = [0x0b, 0x14];
        assert_eq!(ProtoMessage::decode(&mismatched).unwrap_err(), ProtoError::MismatchedGroup { at: 1, expected: 1, found: 2 });
    }

    #[test]
    fn nesting_stops_at_the_limit_and_leaves_the_rest_as_bytes() {
        // A chain of two-byte messages, each holding the next.
        let mut bytes = field_varint(1, 1);
        for _ in 0..(MAX_DEPTH + 5) {
            bytes = field_bytes(1, &bytes);
        }
        let message = ProtoMessage::decode(&bytes).expect("the outer layers still read");
        let mut fields = &message.fields;
        let mut followed = 0;
        while let ProtoValue::Message(inner) = &fields[0].value {
            fields = inner;
            followed += 1;
            assert!(followed <= MAX_DEPTH, "followed {followed} levels, past the limit");
        }
        assert!(followed > 0, "not one level was followed");
        assert_eq!(followed, MAX_DEPTH - 1, "the level past the limit is left as bytes");
        assert!(matches!(fields[0].value, ProtoValue::Blob(_)), "the rest is bytes");
    }

    #[test]
    fn a_value_with_no_blank_body_is_a_blob_not_an_empty_message() {
        let bytes = field_bytes(1, &[]);
        assert_eq!(ProtoMessage::decode(&bytes).unwrap().fields[0].value, ProtoValue::Blob(Vec::new()));
    }

    #[test]
    fn packed_numbers_cannot_be_told_from_a_message_without_the_schema() {
        // A packed `repeated int64` of [8, 150] is the bytes `08 96 01`, and
        // those read as a message with field 1 = 150. Nothing in the bytes says
        // which it is, so the dump shows the message it parses as: a reader with
        // the .proto in hand can tell, and one without it could use neither
        // rendering anyway.
        let packed = field_bytes(6, &[0x08, 0x96, 0x01]);
        assert_eq!(ProtoMessage::decode(&packed).unwrap().render(), "6: {\n  1: 150\n}\n");
    }

    #[test]
    fn a_big_value_is_cut_and_says_so() {
        let big = "a".repeat(TEXT_LIMIT + 10);
        match view(big.as_bytes()) {
            ValueView::Text { text, truncated } => {
                assert!(truncated);
                assert!(text.ends_with('…'));
                assert!(text.len() <= TEXT_LIMIT + '…'.len_utf8());
            }
            other => panic!("not text: {other:?}"),
        }
    }

    /// A value too big to parse is shown as hex, and the hex view is a cut of
    /// it: what a reader sees is not the whole value, and it has to say so.
    #[test]
    fn a_value_past_the_decode_limit_is_measured_not_parsed_and_cut() {
        let bytes = vec![b'x'; MAX_DECODE + 1];
        match view(&bytes) {
            ValueView::Hex { size, text, truncated } => {
                assert_eq!(size, MAX_DECODE + 1);
                assert!(truncated, "a dump of the first {TEXT_LIMIT} bytes is not the value");
                assert!(text.len() <= TEXT_LIMIT + '…'.len_utf8(), "{}", text.len());
            }
            other => panic!("not hex: {other:?}"),
        }
    }

    #[test]
    fn a_cut_never_lands_inside_a_character() {
        let text = "€".repeat(TEXT_LIMIT);
        let (cut, truncated) = cut(&text);
        assert!(truncated);
        assert!(cut.ends_with('…'));
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
    }

    #[test]
    fn the_window_never_sees_the_password() {
        let connection = Connection {
            id: "c1".into(),
            name: "cluster".into(),
            brokers: "broker:9092".into(),
            tls: true,
            ca_file: None,
            sasl: Some(Sasl { mechanism: SaslMechanism::ScramSha512, username: "reports".into(), password: "hunter2".into() }),
            read_only: false,
            agent: true,
        };
        let view = serde_json::to_string(&ConnectionView::from(&connection)).unwrap();
        assert!(!view.contains("hunter2"), "{view}");
        assert!(view.contains("\"login\":{\"mechanism\":\"scramSha512\",\"username\":\"reports\"}"), "{view}");
        assert!(view.contains("\"hasPassword\":true"), "{view}");
        assert!(view.contains("\"agent\":true"), "{view}");
        assert!(view.contains("\"caFile\":null"), "{view}");
    }

    #[test]
    fn a_login_without_a_password_says_there_is_none() {
        let connection = Connection {
            id: "c1".into(),
            name: "cluster".into(),
            brokers: "broker:9092".into(),
            tls: false,
            ca_file: None,
            sasl: Some(Sasl { mechanism: SaslMechanism::Plain, username: "reports".into(), password: String::new() }),
            read_only: true,
            agent: false,
        };
        let view = ConnectionView::from(&connection);
        assert!(!view.has_password);
        assert!(view.read_only);
        assert_eq!(view.login, Some(LoginView { mechanism: SaslMechanism::Plain, username: "reports".into() }));
    }

    #[test]
    fn a_read_only_connection_refuses_to_send_and_says_which_one() {
        let mut connection = Connection {
            id: "c1".into(),
            name: "prod".into(),
            brokers: "broker:9092".into(),
            tls: false,
            ca_file: None,
            sasl: None,
            read_only: true,
            agent: false,
        };
        assert_eq!(connection.producing(), Err(KafkaError::ReadOnly("prod".into())));
        assert_eq!(connection.producing().unwrap_err().to_string(), "\"prod\" is read-only — nothing is sent through it");
        connection.read_only = false;
        assert_eq!(connection.producing(), Ok(()));
    }

    #[test]
    fn lag_is_the_difference_and_is_unknown_without_a_commit() {
        let row = LagView::new("g", "orders", 0, Some(2), 5);
        assert_eq!(row.lag, Some(3));
        let behind = LagView::new("g", "orders", 1, Some(9), 4);
        assert_eq!(behind.lag, Some(0), "a commit past the end is not negative lag");
        let never = LagView::new("g", "orders", 2, None, 7);
        assert_eq!(never.lag, None, "no commit is no data, not zero");
    }

    #[test]
    fn the_last_n_stops_at_the_earliest_offset_still_stored() {
        let partition = PartitionView { partition: 0, leader: Some(1), replicas: vec![1], in_sync: vec![1], earliest: 90, latest: 100 };
        assert_eq!(starting(None, partition.earliest, partition.latest, 20), 90);
        assert_eq!(starting(None, partition.earliest, partition.latest, 5), 95);
        assert_eq!(starting(None, partition.earliest, partition.latest, 0), 100);
        assert_eq!(partition.stored(), 10);
    }

    #[test]
    fn a_topic_says_how_much_is_in_it() {
        let topic = TopicView {
            name: "orders".into(),
            internal: false,
            partitions: vec![
                PartitionView { partition: 0, leader: Some(1), replicas: vec![1], in_sync: vec![1], earliest: 0, latest: 2 },
                PartitionView { partition: 1, leader: Some(1), replicas: vec![1], in_sync: vec![1], earliest: 0, latest: 1 },
            ],
        };
        assert_eq!(topic.stored(), 3);
        assert_eq!(topic.describe(), "orders — 2 partitions, 3 records");
    }

    #[test]
    fn a_record_view_carries_its_place_and_both_halves() {
        let message = Message {
            partition: 3,
            offset: 812,
            timestamp: 1_760_000_000_000,
            key: Some(b"PAY-1".to_vec()),
            value: Some(br#"{"amount":600000}"#.to_vec()),
            headers: vec![("source".into(), Some(b"api".to_vec()))],
        };
        let view = message.view();
        assert_eq!(view.partition, 3);
        assert_eq!(view.offset, 812);
        assert_eq!(view.size, 17);
        assert_eq!(view.key, Some(ValueView::Text { text: "PAY-1".into(), truncated: false }));
        assert!(matches!(view.value, ValueView::Json { .. }));
        assert_eq!(view.headers, vec![HeaderView { name: "source".into(), value: Some("api".into()) }]);
    }

    #[test]
    fn a_record_without_a_value_is_empty_rather_than_text() {
        let view = Message { partition: 0, offset: 0, timestamp: 0, key: None, value: None, headers: vec![] }.view();
        assert_eq!(view.size, 0);
        assert_eq!(view.value, ValueView::Empty);
        assert_eq!(view.key, None);
    }

    #[test]
    fn a_protobuf_message_that_reads_as_text_is_shown_as_protobuf() {
        // One string field of 40 bytes: the tag `0x0a` is a newline to
        // `printable`, the length `0x28` a space, and the rest letters — so the
        // bytes read as text, and are a message only because protobuf is tried
        // first.
        let bytes = field_str(1, &"a".repeat(40));
        assert!(printable(&bytes).is_some(), "the fixture has to read as text");
        assert!(matches!(view(&bytes), ValueView::Protobuf { .. }), "{:?}", view(&bytes));
    }

    #[test]
    fn a_field_number_past_the_protobuf_limit_is_refused_and_named() {
        let too_large = u64::from(MAX_FIELD) + 1;
        let mut bytes = enc_varint(too_large << 3);
        bytes.push(0x01);
        assert_eq!(ProtoMessage::decode(&bytes).unwrap_err(), ProtoError::FieldTooLarge { at: 0, number: too_large });
        // The largest one protobuf has is still a field.
        let mut ok = enc_varint(u64::from(MAX_FIELD) << 3);
        ok.push(0x01);
        assert_eq!(ProtoMessage::decode(&ok).unwrap().fields[0].number, MAX_FIELD);
    }

    #[test]
    fn a_group_reads_every_wire_type_inside_it() {
        let mut inner = field_varint(1, 5);
        inner.extend(enc_tag(2, 1));
        inner.extend(7u64.to_le_bytes());
        inner.extend(enc_tag(3, 5));
        inner.extend(3u32.to_le_bytes());
        inner.extend(field_str(4, "text"));
        let mut bytes = enc_tag(9, 3);
        bytes.extend(inner);
        bytes.extend(enc_tag(9, 4));
        let message = ProtoMessage::decode(&bytes).unwrap();
        let ProtoValue::Group(fields) = &message.fields[0].value else { panic!("not a group") };
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[1].value, ProtoValue::Fixed64(7));
        assert_eq!(fields[2].value, ProtoValue::Fixed32(3));
        assert_eq!(fields[3].value, ProtoValue::Blob(b"text".to_vec()));
    }

    #[test]
    fn a_group_that_is_never_closed_names_the_tag_that_opened_it() {
        let bytes = [0x0b, 0x08, 0x05];
        let error = ProtoMessage::decode(&bytes).unwrap_err();
        assert_eq!(error, ProtoError::UnclosedGroup { at: 0 });
        assert!(error.to_string().contains("never closed"), "{error}");
    }

    #[test]
    fn a_cut_hex_view_says_it_was_cut() {
        match view(&[0x00, 0xff, 0xff]) {
            ValueView::Hex { truncated, .. } => assert!(!truncated),
            other => panic!("not hex: {other:?}"),
        }
        match view(&vec![0xff; TEXT_LIMIT + 10]) {
            ValueView::Hex { truncated, size, .. } => {
                assert!(truncated, "a cut view has to say so");
                assert_eq!(size, TEXT_LIMIT + 10);
            }
            other => panic!("not hex: {other:?}"),
        }
    }

    #[test]
    fn a_newline_inside_a_dumped_string_cannot_break_the_line() {
        let bytes = field_str(1, "two\nlines");
        assert_eq!(ProtoMessage::decode(&bytes).unwrap().render(), "1: \"two\\x0alines\"\n");
    }

    #[test]
    fn the_text_of_a_view_is_what_the_tool_hands_the_model() {
        assert_eq!(view(b"hello").text(), "hello");
        assert_eq!(view(br#"{"a":1}"#).text(), "{\n  \"a\": 1\n}");
        assert_eq!(view(b"").text(), "");
        assert!(view(&example()).text().starts_with("1: \"1260112MOXK#DZ1000122\""));
        assert!(view(&[0x00, 0xff]).text().contains("\\x"));
    }

    #[test]
    fn a_watermark_that_moved_backwards_is_not_negative_and_says_nothing_is_stored() {
        let partition = PartitionView { partition: 0, leader: None, replicas: vec![], in_sync: vec![], earliest: 100, latest: 90 };
        assert_eq!(partition.stored(), 0);
        assert_eq!(starting(None, partition.earliest, partition.latest, 5), 100);
        let topic = TopicView { name: "one".into(), internal: false, partitions: vec![partition] };
        assert_eq!(topic.describe(), "one — 1 partition, 0 records");
    }

    #[test]
    fn a_fixed_field_that_ends_early_is_refused() {
        assert_eq!(ProtoMessage::decode(&[0x09, 0x01, 0x02]).unwrap_err(), ProtoError::Truncated { at: 1 });
    }

    #[test]
    fn a_read_only_connection_refuses_a_write_and_says_why() {
        let connection = Connection {
            id: "c1".into(),
            name: "prod".into(),
            brokers: "localhost:9092".into(),
            tls: false,
            ca_file: None,
            sasl: None,
            read_only: true,
            agent: false,
        };
        let refused = connection.producing().unwrap_err();
        assert_eq!(refused.to_string(), "\"prod\" is read-only — nothing is sent through it");

        let open = Connection { read_only: false, ..connection };
        assert!(open.producing().is_ok());
    }

    #[test]
    fn what_is_sent_is_checked_before_the_broker_sees_it() {
        let spec = |value: &str| ProduceSpec { topic: "orders".into(), partition: None, key: None, headers: vec![], value: value.into() };
        assert!(spec("").checked().is_ok(), "an empty record is a record");
        assert!(spec("a").checked().is_ok());
        assert!(spec(&"x".repeat(MAX_SENT)).checked().is_ok(), "the limit itself is allowed");
        assert!(spec(&"x".repeat(MAX_SENT + 1)).checked().is_err());

        assert!(ProduceSpec { topic: " ".into(), ..spec("v") }.checked().is_err());
        assert!(ProduceSpec { partition: Some(-1), ..spec("v") }.checked().is_err());
        assert!(ProduceSpec { partition: Some(0), ..spec("v") }.checked().is_ok());
        assert!(ProduceSpec { headers: vec![(String::new(), "v".into())], ..spec("v") }.checked().is_err());
        assert!(ProduceSpec { key: Some(String::new()), ..spec("v") }.checked().is_ok(), "an empty key is a key of no bytes");

        // The limit is the record, not its value: a huge key counts too.
        let halves = ProduceSpec { topic: "orders".into(), partition: None, key: Some("k".repeat(MAX_SENT / 2 + 1)), headers: vec![], value: "v".repeat(MAX_SENT / 2 + 1) };
        assert!(halves.checked().is_err(), "the whole record is what a broker measures");
        let crowded = ProduceSpec { headers: (0..MAX_HEADERS + 1).map(|n| (format!("h{n}"), "v".to_string())).collect(), ..spec("v") };
        assert!(crowded.checked().is_err());
        assert!(ProduceSpec { headers: (0..MAX_HEADERS).map(|n| (format!("h{n}"), "v".to_string())).collect(), ..spec("v") }.checked().is_ok());

        // A name the broker never heard of is not a name with a space in it.
        assert_eq!(ProduceSpec { topic: " orders ".into(), ..spec("v") }.checked().unwrap().topic, "orders");
    }

    #[test]
    fn a_read_starts_where_the_log_allows() {
        assert_eq!(starting(None, 90, 100, 20), 90, "the last 20 of a log of 10");
        assert_eq!(starting(None, 0, 500, 20), 480);
        assert_eq!(starting(None, 0, 5, 20), 0, "a log shorter than the count starts at its beginning");
        assert_eq!(starting(Some(-1), 90, 100, 20), 90, "below the log is its start");
        assert_eq!(starting(Some(1000), 90, 100, 20), 100, "past the log is its end");
        assert_eq!(starting(Some(95), 90, 100, 20), 95, "inside it is itself");
        assert_eq!(starting(Some(0), 0, 0, 20), 0, "an empty partition has one place to start");
    }

    #[test]
    fn a_count_is_shared_by_the_partitions_read() {
        assert_eq!(quota(20, 2), 10);
        assert_eq!(quota(20, 3), 7, "rounded up, so three partitions still fill twenty");
        assert_eq!(quota(20, 20), 1);
        assert_eq!(quota(20, 0), 20, "no partition named means no one to share with");
        assert_eq!(quota(1, 20), 1, "never zero: a read asks for at least one message");
    }

    #[test]
    fn a_group_without_a_commit_has_no_lag_rather_than_none_at_all() {
        let never = GroupView::new("g".into(), "Empty".into(), 0, true, vec![LagView::new("g", "t", 0, None, 500)]);
        assert_eq!(never.total_lag, None, "no commit is no data, not caught up");

        let some = GroupView::new(
            "g".into(),
            "Stable".into(),
            2,
            true,
            vec![
                LagView::new("g", "t", 0, Some(90), 100),
                LagView::new("g", "t", 1, Some(30), 100),
                LagView::new("g", "t", 2, Some(900), 100),
            ],
        );
        assert_eq!(some.total_lag, Some(10 + 70), "a commit past the end is no negative lag");
        assert_eq!(some.lags[2].lag, Some(0));
        assert_eq!(some.members, 2);

        // Not asked is not "no commits": the two say different things.
        let unread = GroupView::new("g".into(), "Stable".into(), 2, false, vec![]);
        assert_eq!((unread.lags_read, unread.total_lag), (false, None));

        assert!(is_consumer_group("") && is_consumer_group("consumer"));
        assert!(!is_consumer_group("connect") && !is_consumer_group("transactional"));
    }

    #[test]
    fn a_tick_takes_its_share_and_carries_the_rest() {
        let message = |partition: i32, offset: i64| Message { partition, offset, timestamp: 0, key: None, value: None, headers: vec![] };
        let few = vec![message(1, 5), message(0, 2)];
        let (took, rest) = live_tick(few, 10);
        assert_eq!(rest.len(), 0);
        assert_eq!(took.iter().map(|m| (m.partition, m.offset)).collect::<Vec<_>>(), vec![(0, 2), (1, 5)], "in order, partition then offset");

        let many: Vec<Message> = (0..250).map(|offset| message(0, offset)).collect();
        let (took, rest) = live_tick(many, 200);
        assert_eq!(took.len(), 200);
        assert_eq!(rest.len(), 50, "nothing is dropped: the overflow waits");
        assert_eq!(rest[0].offset, 200);
    }

    #[test]
    fn a_read_is_clamped_to_what_one_page_carries() {
        let spec = ReadSpec { topic: "orders".into(), partitions: (0..30).collect(), from: None, count: 10_000 };
        let clamped = spec.clamped();
        assert_eq!(clamped.partitions.len(), MAX_READ_PARTITIONS);
        assert_eq!(clamped.count, MAX_READ_COUNT);
        assert_eq!(clamped.partitions[0], 0, "the first ones are kept, in order");

        let none = ReadSpec { topic: "orders".into(), partitions: vec![], from: Some(7), count: 0 }.clamped();
        assert_eq!(none.count, 1, "an answer of no messages is not an answer");
        assert_eq!(none.from, Some(7));
    }

    #[test]
    fn what_was_kept_is_fresh_until_its_time_is_up() {
        let at = std::time::Instant::now();
        let ttl = std::time::Duration::from_secs(20);
        assert!(fresh(at, at, ttl));
        assert!(fresh(at, at + std::time::Duration::from_secs(19), ttl));
        assert!(!fresh(at, at + ttl, ttl), "the moment it is up, it is not");
        assert!(!fresh(at, at + std::time::Duration::from_secs(60), ttl));
    }

    #[test]
    fn a_search_matches_part_of_a_name_in_any_case() {
        assert!(matches("orders.created", Some("ORDER")));
        assert!(matches("orders.created", Some(" created ")));
        assert!(matches("orders", Some("orders")));
        assert!(!matches("orders", Some("payments")));
        assert!(matches("orders", None));
        assert!(matches("orders", Some("  ")), "an empty search is no search");
    }

    #[test]
    fn a_page_is_the_search_sorted_and_sliced_with_the_whole_counted() {
        // Handed over in reverse: a page that does not sort shows 249 first.
        let names: Vec<String> = (0..250).rev().map(|n| format!("topic-{n:03}")).collect();
        let (first, total) = page(names.clone(), None, 0, 100);
        assert_eq!((first.len(), total), (100, 250));
        assert_eq!(first.first().map(String::as_str), Some("topic-000"));
        assert_eq!(first.last().map(String::as_str), Some("topic-099"));

        let (second, _) = page(names.clone(), None, 1, 100);
        assert_eq!(second.first().map(String::as_str), Some("topic-100"));
        let (third, _) = page(names.clone(), None, 2, 100);
        assert_eq!(third.len(), 50, "the last page is the remainder");
        assert_eq!(third.first().map(String::as_str), Some("topic-200"));
        let (past, _) = page(names.clone(), None, 9, 100);
        assert!(past.is_empty(), "a page past the end is empty, not a panic");

        let (few, total) = page(names, Some("TOPIC-24"), 0, 100);
        assert_eq!(few.len(), 10, "240…249: {few:?}");
        assert_eq!(total, 10, "the search counts over the whole list");

        let (empty, none) = page(vec!["orders".into()], Some("payments"), 0, 100);
        assert!(empty.is_empty() && none == 0);
    }

    #[test]
    fn a_login_never_prints_its_password() {
        let sasl = Sasl { mechanism: SaslMechanism::Plain, username: "u".into(), password: "hunter2".into() };
        let printed = format!("{sasl:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("•••"), "{printed}");
    }

    /// The window sends what `src/lib/kafka.ts` declares: camelCase keys and a
    /// login with no password when it is an edit that keeps the saved one.
    #[test]
    fn a_connection_is_read_from_the_json_the_window_sends() {
        let json = r#"{"id":"c1","name":"prod","brokers":"h:9092","tls":true,"caFile":"/etc/ca.pem","sasl":{"mechanism":"scramSha512","username":"u","password":""},"readOnly":true,"agent":true}"#;
        let connection: Connection = serde_json::from_str(json).unwrap();
        assert_eq!(connection.ca_file.as_deref(), Some("/etc/ca.pem"));
        assert!(connection.read_only, "a read-only cluster must not arrive writable");
        assert!(connection.agent);
        assert_eq!(connection.sasl.unwrap().mechanism, SaslMechanism::ScramSha512);
    }
}
