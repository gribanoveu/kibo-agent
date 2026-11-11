import { invoke } from "@tauri-apps/api/core";

// The Kafka page's IPC, one wrapper per command in `src-tauri/src/commands/kafka.rs`.

/** Mirrors `domain::kafka::SaslMechanism`. */
export type SaslMechanism = "plain" | "scramSha256" | "scramSha512";

/** Mirrors `domain::kafka::Sasl` — the password only ever travels one way. */
export type Sasl = { mechanism: SaslMechanism; username: string; password: string };

/**
 * Mirrors `domain::kafka::Connection`: what the form sends and the sealed file
 * keeps. An edit with an empty password keeps the saved one; no `sasl` at all
 * turns the login off.
 */
export type Connection = {
  id: string;
  name: string;
  brokers: string;
  tls: boolean;
  caFile: string | null;
  sasl: Sasl | null;
  readOnly: boolean;
  agent: boolean;
};

/** Mirrors `domain::kafka::ConnectionView`: never the password. */
export type ConnectionView = {
  id: string;
  name: string;
  brokers: string;
  tls: boolean;
  caFile: string | null;
  login: { mechanism: SaslMechanism; username: string } | null;
  hasPassword: boolean;
  readOnly: boolean;
  agent: boolean;
};

/** Mirrors `domain::kafka::PartitionView`. `earliest` is the first offset still stored. */
export type PartitionView = {
  partition: number;
  leader: number | null;
  replicas: number[];
  inSync: number[];
  earliest: number;
  latest: number;
};

/** Mirrors `domain::kafka::TopicView`. */
export type TopicView = { name: string; internal: boolean; partitions: PartitionView[] };

/** Mirrors `domain::kafka::TopicList`: `more` are the topics the page did not carry. */
export type TopicList = { topics: TopicView[]; more: number };

/** Mirrors `domain::kafka::ConfigView`. */
export type ConfigView = { name: string; value: string };

/** Mirrors `domain::kafka::TopicDetail`. */
export type TopicDetail = { topic: TopicView; configs: ConfigView[] };

/** What a message's bytes turned out to be (mirrors `domain::kafka::ValueView`). */
export type ValueView =
  | { kind: "empty" }
  | { kind: "json"; text: string; truncated: boolean }
  | { kind: "protobuf"; text: string; schemaId: number | null; truncated: boolean }
  | { kind: "text"; text: string; truncated: boolean }
  | { kind: "hex"; text: string; size: number; truncated: boolean };

/** Mirrors `domain::kafka::MessageView`. */
export type MessageView = {
  partition: number;
  offset: number;
  timestamp: number;
  key: ValueView | null;
  value: ValueView;
  size: number;
  headers: { name: string; value: string | null }[];
};

/** What to read (mirrors `domain::kafka::ReadSpec`): empty `partitions` is all of them. */
export type ReadSpec = { topic: string; partitions: number[]; from: number | null; count: number };

/** Mirrors `domain::kafka::ReadPage`; `more` is the count running out, not the log. */
export type ReadPage = { messages: MessageView[]; partitions: number[]; more: boolean };

/** The text of a value, whichever way it was read. */
export const valueText = (view: ValueView | null): string => (view === null || view.kind === "empty" ? "" : view.text);

export async function kafkaConnections(): Promise<ConnectionView[]> {
  return invoke<ConnectionView[]>("kafka_connections");
}

export async function kafkaConnectionSave(connection: Connection): Promise<ConnectionView[]> {
  return invoke<ConnectionView[]>("kafka_connection_save", { connection });
}

export async function kafkaConnectionDelete(id: string): Promise<ConnectionView[]> {
  return invoke<ConnectionView[]>("kafka_connection_delete", { id });
}

/** What the broker says about itself, for the connection list's Test button. */
export async function kafkaConnectionCheck(id: string): Promise<string> {
  return invoke<string>("kafka_connection_check", { id });
}

/** A page of topics; `name` searches part of one, any case. */
export async function kafkaTopics(connection: string, name?: string | null): Promise<TopicList> {
  return invoke<TopicList>("kafka_topics", { connection, name: name ?? null });
}

export async function kafkaTopic(connection: string, topic: string): Promise<TopicDetail> {
  return invoke<TopicDetail>("kafka_topic", { connection, topic });
}

/** A page of messages, read without a consumer group. */
export async function kafkaMessages(connection: string, spec: ReadSpec): Promise<ReadPage> {
  return invoke<ReadPage>("kafka_messages", { connection, spec });
}
