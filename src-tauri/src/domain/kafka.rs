//! The Kafka page (`docs/54-kafka.md`): what a connection is, what crosses to
//! the window about it, and what a message's bytes are shown as.
//!
//! The message view is the part with rules in it. Kafka carries bytes and
//! nothing else, so the view guesses: JSON if the bytes parse, text if they are
//! printable UTF-8, a raw protobuf dump by field number if they are a wire
//! message, hex otherwise. That dump needs no schema — it is what
//! `protoc --decode_raw` prints — and it is the whole reason this page shows
//! protobuf without a Schema Registry. A length-delimited field is a nested
//! message only if it parses as one; otherwise it is text (when it reads as
//! UTF-8) or escaped bytes.
//!
//! No client types here: `infra::kafka_client` turns krafka's records into
//! [`Message`] and back.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What the window is told about a saved password: that there is one.
pub const MASK: &str = "•••";

/// The most text one message's view carries. Longer values are cut and say so;
/// a value past [`MAX_DECODE`] is not parsed at all, only measured.
pub const TEXT_LIMIT: usize = 64 * 1024;
pub const MAX_DECODE: usize = 1024 * 1024;

/// How deep a length-delimited value is followed before it is left as bytes.
pub const MAX_DEPTH: usize = 64;

// ---------------------------------------------------------------- connection

/// A saved connection, as it is sealed: the password is inside [`Sasl`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sasl {
    pub mechanism: SaslMechanism,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SaslMechanism {
    Plain,
    ScramSha256,
    ScramSha512,
}

impl SaslMechanism {
    /// The name Kafka knows it by.
    pub fn name(self) -> &'static str {
        match self {
            SaslMechanism::Plain => "PLAIN",
            SaslMechanism::ScramSha256 => "SCRAM-SHA-256",
            SaslMechanism::ScramSha512 => "SCRAM-SHA-512",
        }
    }

    /// `None` for a mechanism this app does not speak, which the form refuses
    /// rather than pretending to know.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_uppercase().as_str() {
            "PLAIN" => Some(SaslMechanism::Plain),
            "SCRAM-SHA-256" => Some(SaslMechanism::ScramSha256),
            "SCRAM-SHA-512" => Some(SaslMechanism::ScramSha512),
            _ => None,
        }
    }
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
    /// `SCRAM-SHA-512 as reports`, without the password.
    pub login: Option<String>,
    /// A password is saved: an edit with the field left empty keeps it.
    pub has_password: bool,
    pub read_only: bool,
    pub agent: bool,
}

impl From<&Connection> for ConnectionView {
    fn from(connection: &Connection) -> Self {
        Self {
            id: connection.id.clone(),
            name: connection.name.clone(),
            brokers: connection.brokers.clone(),
            tls: connection.tls,
            login: connection.sasl.as_ref().map(|sasl| format!("{} as {}", sasl.mechanism.name(), sasl.username)),
            has_password: connection.sasl.as_ref().is_some_and(|sasl| !sasl.password.is_empty()),
            read_only: connection.read_only,
            agent: connection.agent,
        }
    }
}

impl Connection {
    /// A name the page can list it under; the form asks for one.
    pub fn named(&self) -> Result<&str, KafkaError> {
        match self.name.trim() {
            "" => Err(KafkaError::NoName),
            name => Ok(name),
        }
    }

    /// Refused before a client is built, so a read-only cluster's only answer
    /// to a send is this sentence rather than a broker's error.
    pub fn producing(&self) -> Result<(), KafkaError> {
        match self.read_only {
            true => Err(KafkaError::ReadOnly(self.named().unwrap_or(&self.name).to_string())),
            false => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KafkaError {
    #[error("a connection needs a name")]
    NoName,
    #[error("\"{0}\" is read-only — nothing is sent through it")]
    ReadOnly(String),
    #[error("a message cannot be read: {0}")]
    BadMessage(String),
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

    /// The window the page reads for "the last `count`": never before the
    /// earliest offset still stored.
    pub fn last(&self, count: i64) -> (i64, i64) {
        let from = (self.latest - count.max(0)).max(self.earliest);
        (from, self.latest)
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
    /// A member holds this partition right now. A group that is all but dead
    /// leaves committed offsets behind, and its lag keeps looking real.
    pub active: bool,
}

impl LagView {
    pub fn new(group: &str, topic: &str, partition: i32, committed: Option<i64>, latest: i64, active: bool) -> Self {
        Self {
            group: group.to_string(),
            topic: topic.to_string(),
            partition,
            committed,
            latest,
            lag: committed.map(|committed| (latest - committed).max(0)),
            active,
        }
    }
}

// ------------------------------------------------------------------ messages

/// One record, as the client hands it over: bytes, nothing interpreted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    pub headers: Vec<(String, String)>,
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
    Hex { text: String, size: usize },
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
            headers: self.headers.iter().map(|(key, value)| (key.clone(), value.as_deref().map_or(String::new(), escaped))).collect(),
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
        return ValueView::Hex { text: escaped(&bytes[..TEXT_LIMIT]), size: bytes.len() };
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
    if let Some(text) = printable(bytes) {
        let (text, truncated) = cut(&text);
        return ValueView::Text { text, truncated };
    }
    if let Ok(message) = ProtoMessage::decode(bytes) {
        let (text, truncated) = cut(&message.render());
        return ValueView::Protobuf { text, schema_id: None, truncated };
    }
    ValueView::Hex { text: escaped(&bytes[..bytes.len().min(TEXT_LIMIT)]), size: bytes.len() }
}

/// A Confluent-framed payload: a zero byte and the schema id, then the message.
/// The id is all that is read of it — the schema itself stays where it is.
pub fn confluent(bytes: &[u8]) -> Option<(u32, &[u8])> {
    match bytes {
        [0, a, b, c, d, rest @ ..] => Some((u32::from_be_bytes([*a, *b, *c, *d]), rest)),
        _ => None,
    }
}

fn cut(text: &str) -> (String, bool) {
    match text.len() > TEXT_LIMIT {
        true => (format!("{}…", &text[..floor_char(text, TEXT_LIMIT)]), true),
        false => (text.to_string(), false),
    }
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
    let awkward = text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'));
    match awkward {
        true => None,
        false => Some(text.to_string()),
    }
}

/// Text with the bytes that are not printable written as `\xNN`, so a binary
/// value is still readable and still exact.
fn escaped(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for character in chunk.valid().chars() {
            match character.is_control() {
                // A NUL or a bare escape byte would be invisible or worse in a
                // `<pre>`: byte-exact means written out.
                true => {
                    let mut buffer = [0u8; 4];
                    for byte in character.encode_utf8(&mut buffer).as_bytes() {
                        out.push_str(&format!("\\x{byte:02x}"));
                    }
                }
                false => out.push(character),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoMessage {
    pub schema_id: Option<u32>,
    pub fields: Vec<ProtoField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoField {
    pub number: u32,
    pub value: ProtoValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoValue {
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
pub enum ProtoError {
    #[error("the message ends inside a value at byte {at}")]
    Truncated { at: usize },
    #[error("a varint at byte {at} is longer than ten bytes")]
    Overlong { at: usize },
    #[error("a tag at byte {at} names field 0, which does not exist")]
    ZeroField { at: usize },
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
}

impl ProtoMessage {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let fields = decode_fields(bytes, 0, MAX_DEPTH)?;
        Ok(Self { schema_id: None, fields })
    }

    /// The dump `protoc --decode_raw` prints, with the one difference this app
    /// wants: text stays text. `9: "Перевод средств"`, not `9: "\320\237…"`.
    pub fn render(&self) -> String {
        let mut out = String::new();
        render_fields(&self.fields, 0, &mut out);
        out
    }
}

/// `None` for a field that ended its group: the end tag belongs to the field
/// that opened it, not to this message.
fn decode_fields(bytes: &[u8], at: usize, depth: usize) -> Result<Vec<ProtoField>, ProtoError> {
    if depth == 0 {
        return Err(ProtoError::TooDeep { max: MAX_DEPTH, at });
    }
    let mut fields = Vec::new();
    let mut at = at;
    while at < bytes.len() {
        let tag_at = at;
        let tag = varint(bytes, &mut at)?;
        let number = tag >> 3;
        let wire = (tag & 0x7) as u8;
        if number == 0 {
            return Err(ProtoError::ZeroField { at: tag_at });
        }
        if number > u32::MAX as u64 {
            return Err(ProtoError::ZeroField { at: tag_at });
        }
        let number = number as u32;
        let value = match wire {
            0 => ProtoValue::Varint(varint(bytes, &mut at)?),
            1 => ProtoValue::Fixed64(fixed(bytes, &mut at, 8)? as u64),
            5 => ProtoValue::Fixed32(fixed(bytes, &mut at, 4)? as u32),
            2 => {
                let length_at = at;
                let length = varint(bytes, &mut at)? as usize;
                let left = bytes.len() - at;
                if length > left {
                    return Err(ProtoError::TooLong { at: length_at, len: length, left });
                }
                let body = &bytes[at..at + length];
                at += length;
                match decode_fields(body, 0, depth - 1) {
                    Ok(fields) if !fields.is_empty() => ProtoValue::Message(fields),
                    _ => ProtoValue::Blob(body.to_vec()),
                }
            }
            3 => {
                let (fields, end) = decode_group(bytes, at, depth - 1, number, tag_at)?;
                at = end;
                ProtoValue::Group(fields)
            }
            // 4 is an end tag, handled where the group began: one here closed
            // no group this message opened.
            4 => return Err(ProtoError::StrayEndGroup { at: tag_at }),
            wire => return Err(ProtoError::BadWireType { at: tag_at, wire }),
        };
        fields.push(ProtoField { number, value });
    }
    Ok(fields)
}

fn decode_group(bytes: &[u8], at: usize, depth: usize, number: u32, opened: usize) -> Result<(Vec<ProtoField>, usize), ProtoError> {
    if depth == 0 {
        return Err(ProtoError::TooDeep { max: MAX_DEPTH, at });
    }
    let mut fields = Vec::new();
    let mut at = at;
    while at < bytes.len() {
        let tag_at = at;
        let tag = varint(bytes, &mut at)?;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x7) as u8;
        if field == 0 {
            return Err(ProtoError::ZeroField { at: tag_at });
        }
        if wire == 4 {
            return match field {
                found if found == number => Ok((fields, at)),
                found => Err(ProtoError::MismatchedGroup { at: tag_at, expected: number, found }),
            };
        }
        // Any other wire type: the group's own field repeats here, so its
        // value is read the same way a message's would be. The tag was already
        // read, so the value starts after it.
        let mut value_at = at;
        let single = decode_field_with_tag(bytes, &mut value_at, field, wire, depth, tag_at)?;
        at = value_at;
        fields.push(single);
    }
    Err(ProtoError::Truncated { at: opened })
}

fn decode_field_with_tag(bytes: &[u8], at: &mut usize, number: u32, wire: u8, depth: usize, tag_at: usize) -> Result<ProtoField, ProtoError> {
    let value = match wire {
        0 => ProtoValue::Varint(varint(bytes, at)?),
        1 => ProtoValue::Fixed64(fixed(bytes, at, 8)? as u64),
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
            match decode_fields(body, 0, depth - 1) {
                Ok(fields) if !fields.is_empty() => ProtoValue::Message(fields),
                _ => ProtoValue::Blob(body.to_vec()),
            }
        }
        3 => {
            let (fields, end) = decode_group(bytes, *at, depth - 1, number, tag_at)?;
            *at = end;
            ProtoValue::Group(fields)
        }
        wire => return Err(ProtoError::BadWireType { at: tag_at, wire }),
    };
    Ok(ProtoField { number, value })
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, ProtoError> {
    let mut value: u64 = 0;
    for index in 0..10 {
        let start = *at;
        let Some(&byte) = bytes.get(*at) else { return Err(ProtoError::Truncated { at: start }) };
        *at += 1;
        match index {
            // The tenth byte carries the one bit that still fits.
            9 if byte > 1 => return Err(ProtoError::Overlong { at: start }),
            9 => value |= (byte as u64) << 63,
            _ => value |= ((byte & 0x7f) as u64) << (7 * index),
        }
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(ProtoError::Overlong { at: *at })
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
            ProtoValue::Blob(bytes) => match printable(bytes) {
                Some(text) => out.push_str(&format!("\"{}\"\n", escape_quotes(&text))),
                None => out.push_str(&format!("\"{}\"\n", escaped(bytes))),
            },
            ProtoValue::Message(fields) => {
                out.push_str("{\n");
                render_fields(fields, depth + 1, out);
                out.push_str(&format!("{indent}}}\n"));
            }
            ProtoValue::Group(fields) => {
                out.push_str("{\n");
                render_fields(fields, depth + 1, out);
                out.push_str(&format!("{indent}}}\n"));
            }
        }
    }
}

fn escape_quotes(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(escaped(&[b'a', 0xff]), "a\\xff");
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
            ValueView::Hex { text, size } => {
                assert_eq!(size, 3);
                assert_eq!(text, "\\x00\\xff\\xff");
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

    #[test]
    fn a_value_past_the_decode_limit_is_measured_and_not_parsed() {
        let bytes = vec![b'x'; MAX_DECODE + 1];
        match view(&bytes) {
            ValueView::Hex { size, .. } => assert_eq!(size, MAX_DECODE + 1),
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
        assert!(view.contains("\"login\":\"SCRAM-SHA-512 as reports\""), "{view}");
        assert!(view.contains("\"hasPassword\":true"), "{view}");
        assert!(view.contains("\"agent\":true"), "{view}");
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
        assert_eq!(view.login.as_deref(), Some("PLAIN as reports"));
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
    fn a_connection_without_a_name_is_refused() {
        let connection = Connection {
            id: "c1".into(),
            name: "  ".into(),
            brokers: "broker:9092".into(),
            tls: false,
            ca_file: None,
            sasl: None,
            read_only: false,
            agent: false,
        };
        assert_eq!(connection.named(), Err(KafkaError::NoName));
        assert_eq!(connection.producing(), Ok(()), "a name is the form's business, not the send's");
    }

    #[test]
    fn a_mechanism_is_read_by_the_name_kafka_uses() {
        assert_eq!(SaslMechanism::from_name("scram-sha-256"), Some(SaslMechanism::ScramSha256));
        assert_eq!(SaslMechanism::from_name(" PLAIN "), Some(SaslMechanism::Plain));
        assert_eq!(SaslMechanism::from_name("GSSAPI"), None);
        assert_eq!(SaslMechanism::ScramSha512.name(), "SCRAM-SHA-512");
    }

    #[test]
    fn lag_is_the_difference_and_is_unknown_without_a_commit() {
        let row = LagView::new("g", "orders", 0, Some(2), 5, true);
        assert_eq!(row.lag, Some(3));
        assert!(row.active);
        let behind = LagView::new("g", "orders", 1, Some(9), 4, false);
        assert_eq!(behind.lag, Some(0), "a commit past the end is not negative lag");
        let never = LagView::new("g", "orders", 2, None, 7, false);
        assert_eq!(never.lag, None, "no commit is no data, not zero");
    }

    #[test]
    fn the_last_n_stops_at_the_earliest_offset_still_stored() {
        let partition = PartitionView { partition: 0, leader: Some(1), replicas: vec![1], in_sync: vec![1], earliest: 90, latest: 100 };
        assert_eq!(partition.last(20), (90, 100));
        assert_eq!(partition.last(5), (95, 100));
        assert_eq!(partition.last(0), (100, 100));
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
        assert_eq!(view.headers, vec![("source".to_string(), "api".to_string())]);
    }

    #[test]
    fn a_record_without_a_value_is_empty_rather_than_text() {
        let view = Message { partition: 0, offset: 0, timestamp: 0, key: None, value: None, headers: vec![] }.view();
        assert_eq!(view.size, 0);
        assert_eq!(view.value, ValueView::Empty);
        assert_eq!(view.key, None);
    }
}
