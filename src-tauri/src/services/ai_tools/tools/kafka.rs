//! `kafka` — the Kafka connections the user opened to the agent, read
//! (`docs/54-kafka.md`, K-9). One tool with an `op`, as `mongo`: where a topic
//! is, and a window of its records.
//!
//! Nothing is written, and no consumer group is created: a read is `assign`,
//! never `subscribe`, so looking at a topic leaves the cluster as it was.
//!
//! The model names a connection by the name the user gave it, never by its
//! brokers, and sees only the ones ticked "Open to the agent". A record comes
//! as the page shows it — JSON, a raw protobuf dump, text, hex — and a JSON
//! value is redacted field by field, as `mongo`'s documents are. A protobuf
//! dump has numbers where names would be, so there is nothing to redact: that
//! is written down in `docs/08-data-policy.md`.

use chrono::DateTime;
use serde_json::{json, Value};

use crate::domain::kube_view::{redact_text, REDACTED};
use crate::domain::kafka::{self, ConnectionView, KafkaError, KafkaServers, Message, MessagePage, ReadSpec, ScanPage, ScanSpec, TopicDetail, ValueView};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{KafkaArgs, KafkaOp, ToolDeps, ToolError, ToolResult};

/// Most topics one answer lists; the rest are counted.
const LISTED: usize = 200;
/// A read's count when `count` is not given, and the most it may be: a window
/// of a topic, not the topic.
const READ_DEFAULT: u32 = 20;
const MOST: u32 = 50;
/// The longest a value is shown to the model; past it, cut and said.
const VALUE_CHARS: usize = 4000;

pub fn kafka(args: &KafkaArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let servers = deps.kafka.as_deref().ok_or(KafkaError::NoneOpen)?;
    let open: Vec<ConnectionView> = servers.connections()?.into_iter().filter(|c| c.agent).collect();
    if open.is_empty() {
        return Err(KafkaError::NoneOpen.into());
    }
    let (text, summary) = match args.op {
        KafkaOp::Topic => topics(servers, &open, args)?,
        KafkaOp::Read => read(servers, &open, args)?,
    };
    Ok(ToolResult::Kafka { text, summary })
}

fn missing(op: KafkaOp, field: &str) -> ToolError {
    ToolError::InvalidArguments { tool: "kafka".into(), reason: format!("op \"{}\" needs `{field}`", op_name(op)) }
}

fn op_name(op: KafkaOp) -> &'static str {
    match op {
        KafkaOp::Topic => "topic",
        KafkaOp::Read => "read",
    }
}

fn names(open: &[ConnectionView]) -> String {
    open.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", ")
}

/// The connection by its name, any case; with none named, the only one open.
fn connection<'a>(open: &'a [ConnectionView], name: Option<&str>, op: KafkaOp) -> Result<&'a ConnectionView, ToolError> {
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return match open {
            [only] => Ok(only),
            _ => Err(ToolError::InvalidArguments {
                tool: "kafka".into(),
                reason: format!("op \"{}\" needs `connection`, one of {}", op_name(op), names(open)),
            }),
        };
    };
    let named: Vec<&ConnectionView> = open.iter().filter(|c| c.name.to_lowercase() == name.to_lowercase()).collect();
    match named[..] {
        [one] => Ok(one),
        [] => Err(KafkaError::NotOpen { name: name.into(), open: names(open) }.into()),
        _ => Err(KafkaError::SameName { name: name.into(), count: named.len() }.into()),
    }
}

/// Without `name` every topic; with it, those whose name has it in them, any
/// case. One match is the topic asked about, and its card says more than a
/// list of one would.
fn topics(servers: &dyn KafkaServers, open: &[ConnectionView], args: &KafkaArgs) -> Result<(String, String), ToolError> {
    let connection = connection(open, args.connection.as_deref(), args.op)?;
    let needle = args.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
    let mut listed = Vec::new();
    let mut total;
    let mut page = 0usize;
    loop {
        let answer = servers.topics(&connection.id, needle, page)?;
        total = answer.total;
        if answer.topics.is_empty() {
            break;
        }
        listed.extend(answer.topics);
        page += 1;
        // Both ends: a cluster smaller than the ceiling is read in one page,
        // and one larger is not read past what an answer carries.
        if listed.len() >= total.min(LISTED) {
            break;
        }
    }
    listed.truncate(LISTED);
    // The topic named whole is the one asked about, even when longer names
    // have it in them: `orders` beside `orders-dlq` would otherwise never
    // answer with its card.
    let exact = needle.and_then(|needle| listed.iter().find(|topic| topic.name == needle));
    let only = listed.first().filter(|_| total == 1);
    if let Some(topic) = exact.or(only) {
        return Ok(card(&servers.topic(&connection.id, &topic.name)?, connection));
    }

    let asked = match needle {
        Some(needle) => format!("Topics of {} whose name has \"{needle}\"", connection.name),
        None => format!("Topics of {}", connection.name),
    };
    if listed.is_empty() {
        return Ok((format!("{asked}: none found — another connection may have it, or the name is spelled differently."), "0 topics".into()));
    }
    let mut text = format!("{asked} ({}):", if total > listed.len() { format!("{} of {total}", listed.len()) } else { total.to_string() });
    for topic in &listed {
        text.push_str(&format!("\n- {}", topic.describe()));
    }
    if total > listed.len() {
        text.push_str(&format!("\n…and {} more — name part of it to narrow the list.", total - listed.len()));
    }
    text.push_str("\n\nName one for its partitions, replicas, in-sync copies, watermarks and configs; op `read` reads its messages.");
    Ok((text, format!("{} topics", listed.len())))
}

fn card(detail: &TopicDetail, connection: &ConnectionView) -> (String, String) {
    let topic = &detail.topic;
    let mut lines: Vec<String> = vec![format!("{} on {}:", topic.describe(), connection.name)];
    for partition in &topic.partitions {
        let behind = if partition.in_sync.len() < partition.replicas.len() { ", behind" } else { "" };
        lines.push(format!(
            "- partition {}: leader {}, replicas {}, in sync {}{behind} — offsets {}..{}, {} records",
            partition.partition,
            partition.leader.map_or_else(|| "not known".to_string(), |leader| leader.to_string()),
            numbers(&partition.replicas),
            numbers(&partition.in_sync),
            partition.earliest,
            partition.latest,
            partition.stored()
        ));
    }
    lines.push(if detail.configs.is_empty() {
        "No config of a reader's kind is set on it: the broker's defaults apply.".to_string()
    } else {
        format!("Configs: {}.", detail.configs.iter().map(|config| format!("{}={}", config.name, config.value)).collect::<Vec<_>>().join(", "))
    });
    (lines.join("\n"), format!("{} partitions, {} records", topic.partitions.len(), topic.stored()))
}

fn numbers(list: &[i32]) -> String {
    if list.is_empty() {
        return "none".to_string();
    }
    list.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
}

fn read(servers: &dyn KafkaServers, open: &[ConnectionView], args: &KafkaArgs) -> Result<(String, String), ToolError> {
    let connection = connection(open, args.connection.as_deref(), args.op)?;
    let topic = args.topic.as_deref().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| missing(args.op, "topic"))?;
    let count = args.count.unwrap_or(READ_DEFAULT).clamp(1, MOST) as usize;
    let partitions = args.partitions.clone().unwrap_or_default();
    match args.key.as_deref().map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => scanned(servers, connection, topic, args, &partitions, key, count),
        None => window(servers, connection, topic, args, &partitions, count),
    }
}

/// The partitions a window read, and what was left out of the list asked for:
/// a topic of a hundred and twenty partitions is not one answer, and a model
/// that asked for all of them has to be told which twenty it got.
fn partitions_read(read: &[i32], asked: &[i32]) -> String {
    if asked.len() <= kafka::MAX_READ_PARTITIONS {
        return numbers(read);
    }
    format!("{}, the first {} of {} asked for", numbers(read), kafka::MAX_READ_PARTITIONS, asked.len())
}

/// A window of the topic: the last `count` records, or those from an offset.
fn window(servers: &dyn KafkaServers, connection: &ConnectionView, topic: &str, args: &KafkaArgs, partitions: &[i32], count: usize) -> Result<(String, String), ToolError> {
    let spec = ReadSpec { topic: topic.to_string(), partitions: partitions.to_vec(), from: args.from, count };
    let page: MessagePage = servers.messages(&connection.id, spec)?;
    let mut messages = page.messages;
    // `count` is the tool's own ceiling: a service that hands back more than it
    // was asked for must not make the answer longer than one page.
    let trimmed = messages.len() > count;
    messages.truncate(count);
    let mut text = format!(
        "{} messages of \"{topic}\" on {} — partitions {}",
        messages.len(),
        connection.name,
        partitions_read(&page.partitions, partitions)
    );
    if let Some(from) = args.from {
        text.push_str(&format!(", from offset {from}"));
    }
    text.push('.');
    if messages.is_empty() {
        text.push_str(" No records in that window.");
    } else {
        text.push_str(&format!("\n{}", render(&messages)));
    }
    // Offsets, not a page number: retention moves the beginning of a topic,
    // so "page 2" is another place by the time it is asked for.
    if page.more || trimmed {
        text.push_str(&format!(
            "\nMore follow. Continue with `from` where a partition stopped — {}. Read one partition at a time to continue it exactly.",
            next(&messages)
        ));
    } else {
        text.push_str("\nNothing follows these on these partitions.");
    }
    Ok((text, format!("{} messages", messages.len())))
}

/// A window looked through for one key. The answer says how much of the log
/// that took, so "not found" is not mistaken for "not there".
fn scanned(servers: &dyn KafkaServers, connection: &ConnectionView, topic: &str, args: &KafkaArgs, partitions: &[i32], key: &str, count: usize) -> Result<(String, String), ToolError> {
    let spec = ScanSpec { topic: topic.to_string(), partitions: partitions.to_vec(), from: args.from, key: key.to_string() };
    let page: ScanPage = servers.scan(&connection.id, spec)?;
    let mut messages = page.messages;
    let cut = messages.len() > count;
    messages.truncate(count);
    let mut text = format!(
        "Scanned {} records of \"{topic}\" on {} for key \"{key}\" — partitions {}",
        page.scanned,
        connection.name,
        partitions_read(&page.partitions, partitions)
    );
    if let Some(from) = args.from {
        text.push_str(&format!(", from offset {from}"));
    }
    text.push('.');
    match (messages.is_empty(), page.more) {
        // Never a bare "not found": the model would read it as "there is none".
        // Nor is a quiet broker the end of a log — what `more` means here is
        // that a partition had records nobody looked at.
        (true, true) => {
            let wider = match args.from {
                Some(_) => "narrow the partitions, or read again from the offset where an earlier `read` window stopped",
                None => "narrow the partitions, or give `from` — an offset a `read` window showed — to start further in",
            };
            text.push_str(&format!(
                " Not found in what it looked at, and the scan stopped before the end of the log (its ceilings: {} records, {} MiB of values, {} s per partition up to {} s in all, or {} matches, whichever came first) — {wider}.",
                kafka::MAX_SCAN,
                kafka::SCAN_BYTES / (1024 * 1024),
                kafka::SCAN_TIME.as_secs(),
                kafka::SCAN_TIME_MOST.as_secs(),
                kafka::MAX_READ_COUNT
            ));
        }
        // An offset narrows what was looked at: "everything stored" would be a lie.
        (true, false) => text.push_str(match args.from {
            Some(_) => " Not found after that offset — the records before it were not looked at, so the key may be there: scan again without `from`, or from an earlier offset.",
            None => " Not found — that is everything stored on those partitions.",
        }),
        (false, _) => {
            let note = match (cut, page.more) {
                (true, true) => format!(" (the first {count}; more matches follow, and the scan stopped before the end of the log)"),
                (true, false) => format!(" (the first {count}; the remaining matches are not shown)"),
                (false, true) => " (there may be more — the scan stopped before the end of the log)".to_string(),
                (false, false) => String::new(),
            };
            text.push_str(&format!(" {} found{note}:\n{}", messages.len(), render(&messages)));
        }
    }
    Ok((text, format!("{} messages", messages.len())))
}

fn render(messages: &[Message]) -> String {
    messages.iter().map(line).collect::<Vec<_>>().join("\n")
}

/// One record as a few header lines and its value under them.
fn line(message: &Message) -> String {
    let mut head = format!("- partition {}, offset {}", message.partition, message.offset);
    if let Some(at) = DateTime::from_timestamp_millis(message.timestamp) {
        head.push_str(&format!(", {}", at.format("%Y-%m-%d %H:%M:%S UTC")));
    }
    if let Some(key) = &message.key {
        head.push_str(&format!(", key \"{}\"", redact_text(&String::from_utf8_lossy(key))));
    }
    let (kind, text) = value(message.value.as_deref());
    head.push_str(&format!(", {kind} value, {} bytes", message.value.as_ref().map_or(0, Vec::len)));
    if !message.headers.is_empty() {
        let headers: Vec<String> = message
            .headers
            .iter()
            .map(|(name, value)| match value {
                Some(bytes) => format!("{name}={}", header(name, bytes)),
                None => name.clone(),
            })
            .collect();
        head.push_str(&format!(", headers {}", headers.join(", ")));
    }
    if text.is_empty() {
        return head;
    }
    // A dump ends with a newline; the indent should not.
    format!("{head}\n  {}", text.trim_end().replace('\n', "\n  "))
}

/// A header's value as the model reads it. Headers are where a token rides
/// along — `Authorization`, a cookie — so one named like that is hidden, and
/// any other goes through the rule a document's field of its name would: a
/// `x-api-key` hidden, a `password=…` inside a value hidden too.
fn header(name: &str, bytes: &[u8]) -> String {
    let lower = name.to_ascii_lowercase();
    if ["authorization", "cookie"].iter().any(|word| lower.contains(word)) {
        return REDACTED.to_string();
    }
    let mut field = json!({ name: String::from_utf8_lossy(bytes) });
    crate::domain::mongo::redact_document(&mut field);
    field[name].as_str().unwrap_or_default().to_string()
}

/// A value as the model reads it, and what kind of thing it is. JSON goes
/// through the same redaction as `mongo`'s documents, and text through the one
/// the cluster's own text gets; a protobuf dump has numbers where field names
/// would be, so there is nothing to redact.
fn value(bytes: Option<&[u8]>) -> (String, String) {
    let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) else {
        return ("empty".to_string(), String::new());
    };
    // JSON is parsed here rather than read out of the view's text: the view
    // cuts at its own limit first, and a cut object is not one to redact. Past
    // that limit nothing is parsed at all, as nothing is parsed for the page.
    if bytes.len() <= kafka::MAX_DECODE {
        if let Ok(mut document) = serde_json::from_slice::<Value>(bytes) {
            crate::domain::mongo::redact_document(&mut document);
            return ("json".to_string(), cut(&serde_json::to_string(&document).unwrap_or_default(), true));
        }
    }
    let view = kafka::view(bytes);
    let (kind, whole) = match &view {
        ValueView::Empty => ("empty", true),
        // Out of reach while the parse above and the view's own are the same
        // parser under the same limit — kept, and redacted below like text, so
        // that a view which starts parsing something else cannot show it whole.
        ValueView::Json { truncated, .. } => ("json", !truncated),
        ValueView::Protobuf { truncated, .. } => ("protobuf", !truncated),
        ValueView::Text { truncated, .. } => ("text", !truncated),
        ValueView::Hex { truncated, .. } => ("hex", !truncated),
    };
    // A form body, a config line: a password in it is hidden by name, as the
    // text of a cluster's own objects is.
    let text = match &view {
        ValueView::Text { text, .. } | ValueView::Json { text, .. } => redact_text(text),
        other => other.text(),
    };
    (kind.to_string(), cut(&text, whole))
}

/// `whole`: the text is all of it, so the count printed is the value's. A view
/// cut before it reached here says only that more follows — a character count
/// of what is left would be a lie told in numbers.
fn cut(text: &str, whole: bool) -> String {
    let total = text.chars().count();
    if total <= VALUE_CHARS {
        return text.to_string();
    }
    let shown: String = text.chars().take(VALUE_CHARS).collect();
    let why = if whole { format!("{total} characters in all") } else { "the value is longer than this".to_string() };
    format!("{shown}… (cut: {why})")
}

/// Where each partition of a window continues: one past its last offset read.
fn next(messages: &[Message]) -> String {
    let mut last: Vec<(i32, i64)> = Vec::new();
    for message in messages {
        match last.iter_mut().find(|(partition, _)| *partition == message.partition) {
            Some(entry) => entry.1 = entry.1.max(message.offset),
            None => last.push((message.partition, message.offset)),
        }
    }
    last.iter().map(|(partition, offset)| format!("partition {partition}: {}", offset + 1)).collect::<Vec<_>>().join(", ")
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kafka".to_string(),
        description: "Reads the user's Kafka clusters — only the connections they opened to you on the Kafka page, by the \
            names they gave them. Nothing is written, and no consumer group is created: a read leaves the cluster as it \
            was. Find a topic first: op `topic` with `name` (a whole name, or part of one, any case) — one match answers \
            with its card, partitions, leaders, replicas, in-sync copies, watermarks and the configs a reader cares about; \
            an exact name answers with its card even when longer names contain it; several matches, or no `name`, list them (up to 200). Then op `read` with `connection` (may be left out when \
            only one is open), `topic`, and: `partitions` (empty is all, up to 20), `from` (an offset; empty reads the last \
            `count`), `count` (up to 50, 20 by default), `key` (only records with that key). Kafka indexes nothing by key: \
            a read with `key` is a scan of the log, bounded to 20 000 records, 8 MiB of values or 5 s per partition (30 s in all), and \
            the answer says what it looked at. A value comes as JSON, as a raw protobuf dump by field number (no schema, \
            so no field names), as text, or as hex. When more follow, the answer names the offset each partition continues \
            from; read one partition at a time to continue exactly."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["topic", "read"] },
                "connection": { "type": ["string", "null"], "description": "The connection's name, as the user gave it. May be left out when only one is open to you." },
                "name": { "type": ["string", "null"], "description": "Only for `topic`: a topic's name, or part of one, any case. Absent lists every topic." },
                "topic": { "type": ["string", "null"], "description": "Only for `read`: the topic to read." },
                "partitions": { "type": ["array", "null"], "items": { "type": "integer" }, "description": "Only for `read`: which partitions of the topic. Empty or absent is all of them, up to 20." },
                "from": { "type": ["integer", "null"], "description": "Only for `read`: the offset to start at. Absent reads the last `count` records." },
                "count": { "type": ["integer", "null"], "description": "Only for `read`: how many records, up to 50; 20 by default." },
                "key": { "type": ["string", "null"], "description": "Only for `read`: only records whose key is this text. A scan of the log, not a lookup." }
            },
            "required": ["op"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::kafka::{ConfigView, PartitionView, TopicList, TopicView};
    use std::sync::{Arc, Mutex};

    /// How many topics one page of the search carries: wider than the page's
    /// own hundred on purpose, so the tool's ceiling is its own and not the
    /// service's word for it.
    const PAGE: usize = 150;

    /// The broker in memory: connections, topic names, the records a read
    /// returns, what a scan says it looked at, and what was asked of it.
    #[derive(Default)]
    struct Fake {
        connections: Vec<ConnectionView>,
        names: Vec<&'static str>,
        messages: Vec<Message>,
        more: bool,
        scanned: Vec<Message>,
        scanned_count: usize,
        scan_more: bool,
        failed: Option<KafkaError>,
        asked: Mutex<Vec<String>>,
    }

    impl Fake {
        fn ask(&self, what: String) {
            self.asked.lock().unwrap().push(what);
        }
        fn answer(&self) -> Result<(), KafkaError> {
            match &self.failed {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    fn topic(name: &str, partitions: usize) -> TopicView {
        TopicView {
            name: name.into(),
            internal: false,
            partitions: (0..partitions)
                .map(|partition| PartitionView {
                    partition: partition as i32,
                    leader: Some(1),
                    replicas: vec![1, 2, 3],
                    in_sync: vec![1, 2],
                    earliest: 0,
                    latest: 10,
                })
                .collect(),
        }
    }

    impl KafkaServers for Fake {
        fn connections(&self) -> Result<Vec<ConnectionView>, KafkaError> {
            Ok(self.connections.clone())
        }
        fn topics(&self, connection: &str, part: Option<&str>, page: usize) -> Result<TopicList, KafkaError> {
            self.ask(format!("topics {connection} {} {page}", part.unwrap_or("*")));
            self.answer()?;
            let (names, total) = kafka::page(self.names.iter().map(|name| name.to_string()).collect(), part, page, PAGE);
            Ok(TopicList { topics: names.into_iter().map(|name| topic(&name, 3)).collect(), total })
        }
        fn topic(&self, connection: &str, name: &str) -> Result<TopicDetail, KafkaError> {
            self.ask(format!("topic {connection} {name}"));
            self.answer()?;
            Ok(TopicDetail {
                topic: topic(name, 2),
                configs: vec![ConfigView { name: "retention.ms".into(), value: "604800000".into() }],
            })
        }
        fn messages(&self, connection: &str, spec: ReadSpec) -> Result<MessagePage, KafkaError> {
            self.ask(format!("messages {connection} {} partitions {:?} from {:?} count {}", spec.topic, spec.partitions, spec.from, spec.count));
            self.answer()?;
            // The service hands back the partitions it read — the spec's own,
            // clamped as the client clamps them — or the topic's, which this
            // broker does not describe.
            let clamped = spec.clamped();
            let partitions = match clamped.partitions.is_empty() {
                true => vec![0, 1],
                false => clamped.partitions,
            };
            Ok(MessagePage { messages: self.messages.clone(), partitions, more: self.more })
        }
        fn scan(&self, connection: &str, spec: ScanSpec) -> Result<ScanPage, KafkaError> {
            self.ask(format!("scan {connection} {} partitions {:?} from {:?} key {}", spec.topic, spec.partitions, spec.from, spec.key));
            self.answer()?;
            let clamped = spec.partitions.clone();
            let partitions = match clamped.is_empty() {
                true => vec![0, 1],
                false => clamped,
            };
            Ok(ScanPage {
                messages: self.scanned.clone(),
                partitions,
                scanned: self.scanned_count,
                more: self.scan_more,
            })
        }
    }

    fn view(id: &str, name: &str, agent: bool) -> ConnectionView {
        ConnectionView {
            id: id.into(),
            name: name.into(),
            brokers: format!("{id}:9092"),
            tls: false,
            ca_file: None,
            login: None,
            has_password: false,
            read_only: true,
            agent,
        }
    }

    /// Local open to the agent, Prod and Staging not.
    fn servers() -> Fake {
        Fake {
            connections: vec![view("c1", "Local", true), view("c2", "Prod", false), view("c3", "Staging", false)],
            names: vec!["orders", "order-events", "payments"],
            ..Fake::default()
        }
    }

    fn run(fake: Fake, args: Value) -> (Result<ToolResult, ToolError>, Vec<String>) {
        let fake = Arc::new(fake);
        let deps = ToolDeps { kafka: Some(fake.clone()), ..ToolDeps::default() };
        let result = kafka(&serde_json::from_value(args).unwrap(), &deps);
        let asked = fake.asked.lock().unwrap().clone();
        (result, asked)
    }

    fn said(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(ToolResult::Kafka { text, .. }) => text,
            other => panic!("{other:?}"),
        }
    }

    fn record(partition: i32, offset: i64, key: &str, value: &[u8]) -> Message {
        Message {
            partition,
            offset,
            timestamp: 1_760_000_000_000,
            key: Some(key.as_bytes().to_vec()),
            value: Some(value.to_vec()),
            headers: vec![("trace".into(), Some(b"abc".to_vec()))],
        }
    }

    #[test]
    fn without_a_name_a_cluster_is_listed_up_to_its_ceiling() {
        let many: Vec<&'static str> = (0..LISTED + 3).map(|n| &*Box::leak(format!("topic{n}").into_boxed_str())).collect();
        let fake = Fake { names: many, ..servers() };
        let text = said(run(fake, json!({ "op": "topic" })).0);
        assert_eq!(text.lines().filter(|line| line.starts_with("- ")).count(), LISTED);
        assert!(text.contains(&format!("({LISTED} of {})", LISTED + 3)), "{text}");
        assert!(text.contains("…and 3 more"), "{text}");
    }

    #[test]
    fn part_of_a_name_finds_a_topic_in_any_case_and_one_match_is_its_card() {
        let (result, asked) = run(servers(), json!({ "op": "topic", "name": "ORDER-EV" }));
        let text = said(result);
        assert_eq!(asked[0], "topics c1 ORDER-EV 0", "the search went to the broker as it was written");
        assert!(text.starts_with("order-events — 2 partitions, 20 records on Local:"), "{text}");
        assert!(asked.contains(&"topic c1 order-events".to_string()), "one match is opened: {asked:?}");

        // Several matches are listed, not opened.
        let (result, asked) = run(servers(), json!({ "op": "topic", "name": "order" }));
        let text = said(result);
        assert!(text.contains("- order-events — 3 partitions, 30 records\n- orders — 3 partitions, 30 records"), "{text}");
        assert!(text.contains("Topics of Local whose name has \"order\" (2):"), "{text}");
        assert!(!asked.iter().any(|a| a.starts_with("topic ")), "{asked:?}");
    }

    /// A whole name is that topic's card, even with longer names containing it.
    #[test]
    fn an_exact_name_is_its_card_beside_longer_names() {
        let (result, asked) = run(Fake { names: vec!["orders", "orders-dlq"], ..servers() }, json!({ "op": "topic", "name": "orders" }));
        assert!(said(result).starts_with("orders — 2 partitions, 20 records on Local:"));
        assert!(asked.contains(&"topic c1 orders".to_string()), "{asked:?}");
    }

    /// A header carries tokens: one named for them is hidden, and so is a
    /// credential inside a key or another header's value.
    #[test]
    fn a_token_in_a_header_or_a_key_is_not_shown() {
        let mut message = record(0, 1, "user password=hunter2", b"plain");
        message.headers = vec![
            ("Authorization".into(), Some(b"Bearer abc.def".to_vec())),
            ("x-api-key".into(), Some(b"k-123".to_vec())),
            ("trace".into(), Some(b"t-1".to_vec())),
        ];
        let text = said(run(Fake { messages: vec![message], ..servers() }, json!({ "op": "read", "topic": "orders" })).0);
        for gone in ["abc.def", "k-123", "hunter2"] {
            assert!(!text.contains(gone), "{gone} in {text}");
        }
        assert!(text.contains("Authorization=<redacted>, x-api-key=<redacted>, trace=t-1"), "{text}");
    }

    #[test]
    fn a_card_carries_the_partitions_watermarks_and_the_configs_a_reader_needs() {
        // The connection's name is matched in any case, as `mongo`'s is.
        let text = said(run(servers(), json!({ "op": "topic", "name": "payments", "connection": "LOCAL" })).0);
        for said in [
            "payments — 2 partitions, 20 records on Local:",
            "partition 0: leader 1, replicas 1, 2, 3, in sync 1, 2, behind — offsets 0..10, 10 records",
            "Configs: retention.ms=604800000.",
        ] {
            assert!(text.contains(said), "{said:?} not in {text}");
        }
    }

    #[test]
    fn a_topic_that_matches_nothing_says_so_and_name_no_topic_is_read() {
        let (result, asked) = run(servers(), json!({ "op": "topic", "name": "billing" }));
        assert_eq!(said(result), "Topics of Local whose name has \"billing\": none found — another connection may have it, or the name is spelled differently.");
        assert!(!asked.iter().any(|a| a.starts_with("topic c1 ")), "{asked:?}");
    }

    #[test]
    fn a_read_is_a_window_with_its_count_claimed_at_fifty() {
        let fake = || Fake {
            messages: vec![record(0, 10, "a", b"{\"n\":1}"), record(1, 40, "b", b"plain")],
            ..servers()
        };
        let (result, asked) = run(fake(), json!({ "op": "read", "topic": "orders", "count": 500, "partitions": [0, 1] }));
        assert_eq!(asked, ["messages c1 orders partitions [0, 1] from None count 50"]);
        let text = said(result);
        assert!(text.starts_with("2 messages of \"orders\" on Local — partitions 0, 1."), "{text}");
        assert!(text.contains("- partition 0, offset 10, 2025-10-09"), "{text}");
        assert!(text.contains("key \"a\", json value, 7 bytes, headers trace=abc\n  {\"n\":1}"), "{text}");
        assert!(text.ends_with("Nothing follows these on these partitions."), "{text}");
    }

    #[test]
    fn an_offset_asked_for_is_the_one_read_and_an_empty_window_is_not_an_error() {
        let (result, asked) = run(servers(), json!({ "op": "read", "topic": "orders", "from": 812 }));
        assert_eq!(asked, ["messages c1 orders partitions [] from Some(812) count 20"]);
        assert_eq!(said(result), "0 messages of \"orders\" on Local — partitions 0, 1, from offset 812. No records in that window.\nNothing follows these on these partitions.");
    }

    #[test]
    fn more_messages_say_where_each_partition_continues() {
        let fake = Fake {
            messages: vec![record(0, 10, "a", b"x"), record(0, 11, "b", b"y"), record(2, 40, "c", b"z")],
            more: true,
            ..servers()
        };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders" })).0);
        assert!(text.contains("More follow. Continue with `from` where a partition stopped — partition 0: 12, partition 2: 41."), "{text}");
    }

    #[test]
    fn a_key_is_a_scan_and_a_scan_that_found_nothing_says_what_it_looked_at() {
        let ceiling = Fake { scanned_count: kafka::MAX_SCAN, scan_more: true, ..servers() };
        let (result, asked) = run(ceiling, json!({ "op": "read", "topic": "orders", "key": "user-1" }));
        assert_eq!(asked, ["scan c1 orders partitions [] from None key user-1"]);
        assert_eq!(
            said(result),
            "Scanned 20000 records of \"orders\" on Local for key \"user-1\" — partitions 0, 1. Not found in what it looked at, and the scan stopped before the end of the log (its ceilings: 20000 records, 8 MiB of values, 5 s per partition up to 30 s in all, or 100 matches, whichever came first) — narrow the partitions, or give `from` — an offset a `read` window showed — to start further in."
        );

        // The whole log read and nothing in it: said plainly, without a ceiling.
        let fake = Fake { scanned_count: 12, scan_more: false, ..servers() };
        assert!(said(run(fake, json!({ "op": "read", "topic": "orders", "key": "user-1" })).0).ends_with("— that is everything stored on those partitions."));
    }

    #[test]
    fn a_scan_that_found_the_key_shows_the_records_and_its_cost() {
        let fake = Fake {
            scanned: vec![record(0, 812, "user-1", b"{\"paid\":true}"), record(1, 900, "user-1", b"x")],
            scanned_count: 1600,
            scan_more: true,
            ..servers()
        };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders", "key": "user-1", "from": "800", "count": 1 })).0);
        assert!(text.starts_with("Scanned 1600 records of \"orders\" on Local for key \"user-1\" — partitions 0, 1, from offset 800. 1 found (the first 1; more matches follow, and the scan stopped before the end of the log):"), "{text}");
        assert!(text.contains("offset 812, 2025-10-09"), "{text}");
        assert!(!text.contains("offset 900"), "the count is what one answer carries: {text}");

        // Not cut by the count, and the log goes on: one line says there may
        // be more, without calling it a ceiling.
        let going = Fake { scanned: vec![record(0, 812, "user-1", b"x")], scanned_count: 40, scan_more: true, ..servers() };
        let text = said(run(going, json!({ "op": "read", "topic": "orders", "key": "user-1" })).0);
        assert!(text.contains("1 found (there may be more — the scan stopped before the end of the log):"), "{text}");
    }

    /// An offset narrows what a scan looked at: "everything stored" would tell
    /// the model the key is nowhere in the topic.
    #[test]
    fn a_scan_after_an_offset_claims_only_what_it_looked_at() {
        let fake = Fake { scanned_count: 40, scan_more: false, ..servers() };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders", "key": "user-1", "from": 900 })).0);
        assert!(text.contains("from offset 900"), "{text}");
        assert!(text.ends_with("— the records before it were not looked at, so the key may be there: scan again without `from`, or from an earlier offset."), "{text}");
    }

    /// The window is the tool's own page, whatever the service hands back.
    #[test]
    fn a_window_is_cut_to_what_one_answer_carries() {
        let fake = Fake { messages: (0..60).map(|n| record(0, n, "a", b"x")).collect(), ..servers() };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders" })).0);
        assert_eq!(text.lines().filter(|line| line.starts_with("- partition")).count(), READ_DEFAULT as usize);
        assert!(text.starts_with("20 messages of \"orders\""), "{text}");
        assert!(text.contains("More follow. Continue with `from` where a partition stopped — partition 0: 20."), "{text}");
    }

    #[test]
    fn a_json_value_has_its_secret_fields_redacted_and_a_long_one_is_cut() {
        let secret = br#"{"user":"ann","password":"hunter2","apiKey":"k1","note":"x"}"#;
        let long = format!("{{\"note\":\"{}\"}}", "y".repeat(VALUE_CHARS + 10));
        // A view cut at the domain's own decode limit says it is longer, not
        // how long the remains are: the count would be of the wrong text.
        let huge = "z".repeat(2 * 1024 * 1024);
        let fake = Fake {
            messages: vec![record(0, 1, "a", secret), record(1, 2, "b", long.as_bytes()), record(2, 3, "c", huge.as_bytes())],
            ..servers()
        };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders" })).0);
        assert!(!text.contains("hunter2") && !text.contains("\"k1\""), "{text}");
        assert!(text.contains("\"password\":\"<redacted>\""), "{text}");
        // The JSON that fits says how much of it there was; the one the domain
        // cut at its own limit says only that more follows.
        assert!(text.contains(&format!("… (cut: {} characters in all)", VALUE_CHARS + 21)), "{text}");
        assert!(text.contains("… (cut: the value is longer than this)"), "{text}");
    }

    #[test]
    fn a_value_that_is_not_json_is_shown_the_way_the_page_shows_it() {
        let fake = Fake {
            messages: vec![
                record(0, 1, "a", b"just text"),
                record(1, 2, "b", &[0x08, 0x96, 0x01]),
                // A form body carries a secret as plainly as a JSON one.
                record(2, 3, "c", b"user=ann&password=hunter2"),
            ],
            ..servers()
        };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders" })).0);
        assert!(text.contains(", text value, 9 bytes, headers trace=abc\n  just text"), "{text}");
        assert!(text.contains(", protobuf value, 3 bytes, headers trace=abc\n  1: 150"), "{text}");
        assert!(!text.contains("hunter2") && text.contains("password=<redacted>"), "{text}");
    }

    #[test]
    fn only_a_connection_opened_to_the_agent_is_reached_by_its_name() {
        let (result, asked) = run(servers(), json!({ "op": "read", "connection": "Prod", "topic": "orders" }));
        let error = result.unwrap_err().to_string();
        assert!(error.contains("no connection named \"Prod\" is open") && error.contains("\"Local\""), "{error}");
        assert!(!error.contains("Staging"), "a connection that is not open is not listed: {error}");
        assert!(asked.is_empty(), "{asked:?}");

        let closed = Fake { connections: vec![view("c2", "Prod", false)], ..servers() };
        assert!(matches!(run(closed, json!({ "op": "topic" })).0, Err(ToolError::Kafka(KafkaError::NoneOpen))));
        let off = kafka(&serde_json::from_value(json!({ "op": "topic" })).unwrap(), &ToolDeps::default());
        assert!(matches!(off, Err(ToolError::Kafka(KafkaError::NoneOpen))));
    }

    #[test]
    fn the_connection_may_be_left_out_only_when_one_is_open() {
        let one = Fake { connections: vec![view("c1", "Local", true), view("c2", "Prod", false)], ..servers() };
        assert!(said(run(one, json!({ "op": "read", "topic": "orders" })).0).starts_with("0 messages of \"orders\" on Local"));

        let two = Fake { connections: vec![view("c1", "Local", true), view("c3", "Staging", true)], ..servers() };
        let error = run(two, json!({ "op": "read", "topic": "orders" })).0.unwrap_err().to_string();
        assert!(error.contains("needs `connection`, one of \"Local\", \"Staging\""), "{error}");
    }

    #[test]
    fn a_read_needs_its_topic_and_nothing_is_asked_before_it_has_one() {
        let (result, asked) = run(servers(), json!({ "op": "read" }));
        assert!(matches!(result, Err(ToolError::InvalidArguments { .. })), "{result:?}");
        assert!(asked.is_empty(), "{asked:?}");
    }

    /// A model that means "every partition" often writes `null` for the list,
    /// which is how the schema describes it — and a whole call was lost on it.
    #[test]
    fn no_partitions_and_null_both_read_them_all() {
        for args in [json!({ "op": "read", "topic": "orders", "partitions": null }), json!({ "op": "read", "topic": "orders" })] {
            let (result, asked) = run(servers(), args);
            assert_eq!(asked, ["messages c1 orders partitions [] from None count 20"]);
            assert!(said(result).starts_with("0 messages of \"orders\" on Local — partitions 0, 1."));
        }
    }

    /// A topic of a hundred and twenty partitions is not one answer: the model
    /// is told which twenty it got, or it would believe all of them were read.
    #[test]
    fn a_topic_of_many_partitions_says_which_twenty_it_read() {
        let many: Vec<i32> = (0..21).collect();
        let text = said(run(servers(), json!({ "op": "read", "topic": "orders", "partitions": many })).0);
        assert!(
            text.starts_with("0 messages of \"orders\" on Local — partitions 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, the first 20 of 21 asked for."),
            "{text}"
        );
    }

    /// A value past the page's decode limit is not parsed as JSON by the tool
    /// either: the page shows hex, and the model reads the same bytes.
    #[test]
    fn a_value_past_the_decode_limit_is_hex_for_the_model_too() {
        let huge = format!("{{\"note\":\"{}\"}}", "a".repeat(kafka::MAX_DECODE));
        let fake = Fake { messages: vec![record(0, 1, "a", huge.as_bytes())], ..servers() };
        let text = said(run(fake, json!({ "op": "read", "topic": "orders" })).0);
        assert!(text.contains(", hex value, "), "{text}");
        assert!(text.contains("… (cut: the value is longer than this)"), "{text}");
    }

    /// A cluster refuses a read by its own rights, and what the broker said is
    /// what the model reads — the app knows nothing of the account's grants.
    #[test]
    fn what_the_broker_refuses_is_said_in_its_own_words() {
        let fake = Fake { failed: Some(KafkaError::Broker("TOPIC_AUTHORIZATION_FAILED".into())), ..servers() };
        let error = run(fake, json!({ "op": "read", "topic": "orders" })).0.unwrap_err().to_string();
        assert_eq!(error, "TOPIC_AUTHORIZATION_FAILED");
    }
}
