import { useEffect, useState } from "react";
import { ArrowLeft, Check, Database, Pencil, Plus, Radio, RefreshCw, Search, Send } from "lucide-react";
import "./KafkaPage.css";
import { Modal } from "./Modal";
import { KafkaIcon } from "./KafkaIcon";
import { Page, PageEmpty } from "./Page";
import { Switch } from "./Switch";
import { Tabs } from "./Tabs";
import { Dropdown } from "./Dropdown";
import { useKafka } from "../hooks/useKafka";
import { COUNTS, LIVE_PARTITIONS, useKafkaTopic } from "../hooks/useKafkaTopic";
import { valueText, TOPIC_PAGE, type Connection, type ConnectionView, type GroupList, type MessageView, type ProduceSpec, type SaslMechanism, type TopicDetail, type TopicView } from "../lib/kafka";

/** A connection as the form edits it: the password apart, so an empty one can mean "keep". */
type Draft = {
  id: string;
  name: string;
  brokers: string;
  tls: boolean;
  caFile: string;
  login: boolean;
  mechanism: SaslMechanism;
  username: string;
  password: string;
  hasPassword: boolean;
  readOnly: boolean;
  agent: boolean;
};

const MECHANISMS: { value: SaslMechanism; label: string; hint: string }[] = [
  { value: "scramSha512", label: "SCRAM-SHA-512", hint: "The usual choice for a modern cluster" },
  { value: "scramSha256", label: "SCRAM-SHA-256", hint: "Older clusters and some managed ones" },
  { value: "plain", label: "PLAIN", hint: "Sends the password to the broker as it is — only over TLS" },
];

const blank = (): Draft => ({
  id: crypto.randomUUID(),
  name: "",
  brokers: "",
  tls: false,
  caFile: "",
  login: false,
  mechanism: "scramSha512",
  username: "",
  password: "",
  hasPassword: false,
  readOnly: false,
  agent: false,
});

const fromSaved = (connection: ConnectionView): Draft => ({
  id: connection.id,
  name: connection.name,
  brokers: connection.brokers,
  tls: connection.tls,
  caFile: connection.caFile ?? "",
  login: connection.login !== null,
  mechanism: connection.login?.mechanism ?? "scramSha512",
  username: connection.login?.username ?? "",
  password: "",
  hasPassword: connection.hasPassword,
  readOnly: connection.readOnly,
  agent: connection.agent,
});

const toConnection = (draft: Draft): Connection => ({
  id: draft.id,
  name: draft.name.trim(),
  brokers: draft.brokers.trim(),
  tls: draft.tls,
  caFile: draft.tls && draft.caFile.trim() !== "" ? draft.caFile.trim() : null,
  sasl: draft.login ? { mechanism: draft.mechanism, username: draft.username.trim(), password: draft.password } : null,
  readOnly: draft.readOnly,
  agent: draft.agent,
});

const count = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

export function KafkaPage() {
  const kafka = useKafka();
  const [editing, setEditing] = useState<Draft | null>(null);
  const [dropping, setDropping] = useState<ConnectionView | null>(null);
  const open = kafka.open;

  return (
    <Page
      title="Kafka"
      bar={
        kafka.connections.length > 0 && (
          <>
            <Dropdown
              label={
                <span className="kafka-connection-label">
                  <Radio size={14} />
                  {open?.name ?? "Connection"}
                </span>
              }
              title={open?.brokers}
              heading="Connection"
              options={kafka.connections.map((connection) => ({ value: connection.id, label: connection.name, hint: connection.brokers }))}
              value={kafka.selected ?? ""}
              onPick={kafka.select}
              below
            />
            {open && (
              <button type="button" className="btn btn-ghost" title="Change this connection" onClick={() => setEditing(fromSaved(open))}>
                <Pencil size={13} /> Edit
              </button>
            )}
            <button type="button" className="btn btn-ghost" title="New connection" onClick={() => setEditing(blank())}>
              <Plus size={14} />
              Connection
            </button>

          </>
        )
      }
    >
      <div className="kafka-body">
        {/* A save's refusal belongs in the form that asked; the body says the
            rest — a read, a list — and not under a modal's overlay. */}
        {kafka.error && !editing && (
          <p className="kafka-error" role="alert">
            {kafka.error}
          </p>
        )}

        {!open && (
          <PageEmpty
            icon={KafkaIcon}
            action={
              <button type="button" className="btn btn-primary" onClick={() => setEditing(blank())}>
                <Plus size={14} /> Add connection
              </button>
            }
          >
            No connections yet. Add one with the brokers of a cluster, and a login if it wants one — the password is sealed on this computer as the API keys
            are, and this window never sees it again.
          </PageEmpty>
        )}

        {open && (
          <>
            <header className="kafka-main-head">
              <h2 className="kafka-title" title={open.brokers}>
                {open.name}
              </h2>
              <span className="kafka-spacer" />
              <span className="kafka-tags">
                {open.readOnly && <span className="kafka-tag">read-only</span>}
                {open.tls && <span className="kafka-tag">TLS</span>}
                {open.login && <span className="kafka-tag">{open.login.mechanism === "plain" ? "PLAIN" : "SCRAM"}</span>}
                {open.agent && <span className="kafka-tag kafka-tag-agent">agent</span>}
              </span>
            </header>

            {kafka.detail ? (
              <Topic detail={kafka.detail} connection={open.id} readOnly={open.readOnly} onBack={kafka.closeTopic} />
            ) : (
              <>
                <div className="kafka-search">
                  <Search size={13} />
                  <input
                    type="search"
                    value={kafka.search}
                    aria-label="Find a topic"
                    placeholder="Find a topic by part of its name"
                    onChange={(event) => kafka.setSearch(event.target.value)}
                  />
                  {kafka.reading && (
                    <span className="kafka-quiet" role="status">
                      Reading…
                    </span>
                  )}
                  <button
                    type="button"
                    className="kafka-icon-button"
                    title="Read the topics again — a new one appears now, not in two minutes"
                    aria-label="Read the topics again"
                    disabled={kafka.reading}
                    onClick={() => kafka.refresh()}
                  >
                    <RefreshCw size={13} />
                  </button>
                </div>

                <div className="kafka-topics">
                  {kafka.topics.length === 0 && !kafka.reading && !kafka.error && (
                    <p className="kafka-quiet">{kafka.search.trim() ? "No topic answers that." : "This cluster has no topics."}</p>
                  )}

                  {kafka.topics.length > 0 && (
                    <table className="kafka-table">
                      <thead>
                        <tr>
                          <th scope="col">Topic</th>
                          <th scope="col">Partitions</th>
                          <th scope="col">Records</th>
                        </tr>
                      </thead>
                      <tbody>
                        {kafka.topics.map((topic) => (
                          <tr key={topic.name}>
                            <td>
                              <button type="button" className="kafka-topic" onClick={() => void kafka.openTopic(topic.name)}>
                                {topic.internal && <Database size={12} className="kafka-topic-icon" />}
                                {topic.name}
                              </button>
                            </td>
                            <td>{topic.partitions.length}</td>
                            <td>{topic.partitions.reduce((sum, partition) => sum + Math.max(0, partition.latest - partition.earliest), 0)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}

                  {kafka.topics.length === 0 && kafka.total > 0 && <p className="kafka-quiet">Nothing on this page — go back a page.</p>}

                  {kafka.groups && <Groups list={kafka.groups} open={kafka.group} onOpen={kafka.openGroup} />}
                </div>

                {kafka.total > TOPIC_PAGE && (
                  <div className="kafka-pager">
                    <button type="button" className="btn btn-ghost" disabled={kafka.pageAt === 0 || kafka.reading} onClick={() => kafka.setPage(kafka.pageAt - 1)}>
                      ‹ Previous
                    </button>
                    <span className="kafka-quiet">
                      {`${count(kafka.total, "topic")} — ${kafka.pageAt * TOPIC_PAGE + 1}–${kafka.pageAt * TOPIC_PAGE + kafka.topics.length} shown`}
                    </span>
                    <button
                      type="button"
                      className="btn btn-ghost"
                      disabled={(kafka.pageAt + 1) * TOPIC_PAGE >= kafka.total || kafka.reading}
                      onClick={() => kafka.setPage(kafka.pageAt + 1)}
                    >
                      Next ›
                    </button>
                  </div>
                )}
              </>
            )}
          </>
        )}
      </div>

      <ConnectionForm
        draft={editing}
        existing={editing !== null && kafka.connections.some((connection) => connection.id === editing.id)}
        fault={kafka.error}
        onSave={kafka.save}
        onProbe={kafka.probe}
        onDelete={() => {
          const id = editing?.id;
          setEditing(null);
          const saving = kafka.connections.find((connection) => connection.id === id);
          if (saving) setDropping(saving);
        }}
        onClose={() => setEditing(null)}
      />

      <Modal
        title="Delete this connection?"
        open={dropping !== null}
        onClose={() => setDropping(null)}
        footer={
          <>
            <button type="button" className="btn btn-ghost" onClick={() => setDropping(null)}>
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                if (dropping) void kafka.remove(dropping.id);
                setDropping(null);
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p>“{dropping?.name}” and the password saved with it go from this computer. The cluster itself is not touched.</p>
      </Modal>
    </Page>
  );
}

/** The cluster's consumer groups, and the lag of the one opened. */
function Groups({ list, open, onOpen }: { list: GroupList; open: string | null; onOpen: (name: string | null) => void }) {
  const shown = list.groups.find((group) => group.name === open) ?? null;
  return (
    <section className="kafka-groups" aria-label="Consumer groups">
      <span className="kafka-section-title">Consumer groups</span>
      {list.groups.length === 0 && <p className="kafka-quiet">This cluster has no consumer groups.</p>}
      {list.total > list.groups.length && <p className="kafka-quiet">{`Showing ${list.groups.length} of ${list.total} groups.`}</p>}
      {list.unreadBrokers > 0 && (
        <p className="kafka-warn">{`${list.unreadBrokers} ${list.unreadBrokers === 1 ? "broker did" : "brokers did"} not answer — their groups are missing from this list, not from the cluster.`}</p>
      )}

      {list.groups.length > 0 && (
        <table className="kafka-table">
          <thead>
            <tr>
              <th scope="col">Group</th>
              <th scope="col">State</th>
              <th scope="col">Members</th>
              <th scope="col">Lag</th>
            </tr>
          </thead>
          <tbody>
            {list.groups.map((group) => (
              <tr key={group.name} className={group.name === open ? "on" : undefined}>
                <td>
                  <button type="button" className="kafka-topic" onClick={() => onOpen(group.name === open ? null : group.name)}>
                    {group.name}
                  </button>
                </td>
                <td>{group.state}</td>
                <td>{group.members}</td>
                <td className={group.totalLag === null ? "kafka-quiet" : undefined}>
                  {group.totalLag !== null ? group.totalLag : group.lagsRead ? "no commits" : "not read"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {shown && shown.lagsRead && shown.lags.length > 0 && (
        <table className="kafka-table">
          <thead>
            <tr>
              <th scope="col">Topic</th>
              <th scope="col">Partition</th>
              <th scope="col">Committed</th>
              <th scope="col">End</th>
              <th scope="col">Lag</th>
            </tr>
          </thead>
          <tbody>
            {shown.lags.map((lag) => (
              <tr key={`${lag.topic}/${lag.partition}`}>
                <td className="kafka-mono">{lag.topic}</td>
                <td>{lag.partition}</td>
                <td>{lag.committed ?? "—"}</td>
                <td>{lag.latest}</td>
                <td className={lag.lag === null ? "kafka-quiet" : lag.lag > 0 ? "kafka-warn" : undefined}>{lag.lag ?? "no commit"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {shown && !shown.lagsRead && <p className="kafka-quiet">The lag of this group was not read — this page reads the first twenty groups' lag.</p>}
    </section>
  );
}

/** One topic opened: its partitions, its messages and its configs, a tab each. */
function Topic({ detail, connection, readOnly, onBack }: { detail: TopicDetail; connection: string; readOnly: boolean; onBack: () => void }) {
  const [view, setView] = useState<"partitions" | "messages" | "configs">("messages");
  const total = detail.topic.partitions.reduce((sum, partition) => sum + Math.max(0, partition.latest - partition.earliest), 0);

  return (
    <div className="kafka-topic-panel">
      <div className="kafka-topic-head">
        <button type="button" className="kafka-icon-button" title="Back to the topics" aria-label="Back to the topics" onClick={onBack}>
          <ArrowLeft size={14} />
        </button>
        <h3 className="kafka-topic-name">{detail.topic.name}</h3>
        <span className="kafka-quiet">{`${count(detail.topic.partitions.length, "partition")}, ${count(total, "record")}`}</span>
      </div>

      <Tabs<"partitions" | "messages" | "configs">
        label="What to show of the topic"
        tabs={[
          { id: "messages", label: "Messages" },
          { id: "partitions", label: "Partitions", count: detail.topic.partitions.length },
          ...(detail.configs.length > 0 ? [{ id: "configs" as const, label: "Configs", count: detail.configs.length }] : []),
        ]}
        value={view}
        onChange={setView}
      />

      <div className="kafka-topic-body" role="tabpanel">
        {view === "partitions" && (
          <table className="kafka-table">
            <thead>
              <tr>
                <th scope="col">Partition</th>
                <th scope="col">Leader</th>
                <th scope="col">Replicas</th>
                <th scope="col">In sync</th>
                <th scope="col">First offset</th>
                <th scope="col">Next offset</th>
                <th scope="col">Records</th>
              </tr>
            </thead>
            <tbody>
              {detail.topic.partitions.map((partition) => (
                <tr key={partition.partition}>
                  <td>{partition.partition}</td>
                  <td>{partition.leader ?? "—"}</td>
                  <td className="kafka-mono">{partition.replicas.join(", ") || "—"}</td>
                  <td className="kafka-mono">
                    {partition.inSync.join(", ") || "—"}
                    {partition.inSync.length < partition.replicas.length && <span className="kafka-warn"> behind</span>}
                  </td>
                  <td>{partition.earliest}</td>
                  <td>{partition.latest}</td>
                  <td>{Math.max(0, partition.latest - partition.earliest)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}

        {view === "messages" && <Messages connection={connection} topic={detail.topic} readOnly={readOnly} />}

        {view === "configs" && (
          <dl className="kafka-configs">
            {detail.configs.map((config) => (
              <div key={config.name} className="kafka-config">
                <dt>{config.name}</dt>
                <dd className="kafka-mono">{config.value || "—"}</dd>
              </div>
            ))}
          </dl>
        )}
      </div>
    </div>
  );
}

/**
 * The topic's messages: which partitions to read, from where and how many, and
 * one message opened — its key, headers and value in whichever view its bytes
 * turned out to be. What it does lives in `useKafkaTopic`.
 */
function Messages({ connection, topic, readOnly }: { connection: string; topic: TopicView; readOnly: boolean }) {
  const messages = useKafkaTopic({ connection, topic });
  // A live read follows the partitions it started with: another window chosen
  // under it would be overwritten by its next tick.
  const held = messages.living || messages.starting;

  return (
    <section className="kafka-messages" aria-label={`Messages of ${topic.name}`}>
      <div className="kafka-message-bar">
        <span className="kafka-section-title">Messages</span>
        <div className="kafka-partitions" role="group" aria-label="Partitions to read">
          <button
            type="button"
            className={`kafka-chip${messages.partitions.length === 0 ? " on" : ""}`}
            aria-pressed={messages.partitions.length === 0}
            disabled={held}
            onClick={messages.allPartitions}
          >
            All
          </button>
          {topic.partitions.map((partition) => (
            <button
              key={partition.partition}
              type="button"
              className={`kafka-chip${messages.partitions.includes(partition.partition) ? " on" : ""}`}
              aria-pressed={messages.partitions.includes(partition.partition)}
              disabled={held}
              onClick={() => messages.togglePartition(partition.partition)}
            >
              {partition.partition}
            </button>
          ))}
        </div>
        <span className="kafka-spacer" />
        <button
          type="button"
          className={`kafka-chip${messages.living ? " on" : ""}`}
          aria-pressed={messages.living}
          disabled={messages.starting}
          title={messages.living ? "Stop following this topic" : "Follow this topic as messages arrive"}
          onClick={() => void messages.toggleLive()}
        >
          Live
        </button>
        {COUNTS.map((howMany) => (
          <button
            key={howMany}
            type="button"
            className={`kafka-chip${messages.count === howMany ? " on" : ""}`}
            disabled={messages.reading || held}
            onClick={() => messages.setCount(howMany)}
          >
            Last {howMany}
          </button>
        ))}
        <input
          className="kafka-offset"
          aria-label="Read from offset"
          placeholder="from offset"
          value={messages.from}
          onChange={(event) => messages.setFrom(event.target.value)}
        />
        <button type="button" className="btn btn-ghost" disabled={messages.reading || held} onClick={() => void messages.ask()}>
          Read
        </button>
        <button
          type="button"
          className="btn btn-ghost"
          disabled={readOnly}
          title={readOnly ? "This connection is read-only — nothing is sent through it" : "Send a message to this topic"}
          onClick={messages.openSend}
        >
          <Send size={13} /> Send…
        </button>
      </div>

      {topic.partitions.length > LIVE_PARTITIONS && messages.partitions.length === 0 && (
        <p className="kafka-quiet">This topic has {topic.partitions.length} partitions and one answer spans {LIVE_PARTITIONS} — pick the ones to read.</p>
      )}
      {messages.error && <p className="kafka-error" role="alert">{messages.error}</p>}
      {messages.sent && <p className="kafka-ok">{messages.sent}</p>}
      {messages.living && messages.waiting > 0 && <p className="kafka-quiet">{messages.waiting} more arrived and are on their way.</p>}
      {messages.living && messages.partitions.length === 0 && topic.partitions.length > LIVE_PARTITIONS && (
        <p className="kafka-quiet">{`Following the first ${LIVE_PARTITIONS} of ${topic.partitions.length} partitions — pick the ones to read.`}</p>
      )}
      {readOnly && <p className="kafka-quiet">This connection is read-only — nothing is sent through it.</p>}
      {messages.reading && <p className="kafka-quiet" role="status">Reading…</p>}

      {messages.page && !messages.reading && !messages.error && messages.page.messages.length === 0 && (
        <p className="kafka-quiet">Nothing at that offset in the chosen partitions.</p>
      )}

      {messages.page && messages.page.messages.length > 0 && (
        <table className="kafka-table">
          <thead>
            <tr>
              <th scope="col">Partition</th>
              <th scope="col">Offset</th>
              <th scope="col">Key</th>
              <th scope="col">Bytes</th>
              <th scope="col">Time</th>
            </tr>
          </thead>
          <tbody>
            {messages.page.messages.map((message) => {
              const on = messages.opened?.partition === message.partition && messages.opened.offset === message.offset;
              return (
                <tr key={`${message.partition}/${message.offset}`} className={on ? "on" : undefined}>
                  <td>{message.partition}</td>
                  <td>
                    <button
                      type="button"
                      className="kafka-topic"
                      aria-label={`Message ${message.partition}/${message.offset}`}
                      onClick={() => messages.open(on ? null : message)}
                    >
                      {message.offset}
                    </button>
                  </td>
                  <td className="kafka-mono kafka-key">{valueText(message.key) || "—"}</td>
                  <td>{message.size}</td>
                  <td>{new Date(message.timestamp).toLocaleString()}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {messages.page?.more && <p className="kafka-quiet">The count ran out before the log did — read on from the last offset.</p>}

      {messages.opened && <MessageBody message={messages.opened} />}

      <SendForm
        open={messages.writing}
        topic={topic.name}
        onClose={messages.closeSend}
        partitionCount={topic.partitions.length}
        onSend={messages.send}
      />
    </section>
  );
}

/** One message opened: its headers and its value, as its bytes were read. */
function MessageBody({ message }: { message: MessageView }) {
  const view = message.value;
  return (
    <div className="kafka-message">
      <div className="kafka-message-head">
        <span className="kafka-mono">
          {message.partition}/{message.offset}
        </span>
        <span className="kafka-tag">{view.kind}</span>
        <span className="kafka-quiet">{new Date(message.timestamp).toLocaleString()}</span>
      </div>
      {message.headers.length > 0 && (
        <dl className="kafka-configs">
          {message.headers.map((header, index) => (
            <div key={`${header.name}/${index}`} className="kafka-config">
              <dt>{header.name}</dt>
              <dd className="kafka-mono">{header.value ?? "—"}</dd>
            </div>
          ))}
        </dl>
      )}
      <pre className={`kafka-value${view.kind === "hex" ? " kafka-mono" : ""}`}>{view.kind === "empty" ? "(no value)" : valueText(view)}</pre>
      {(view.kind === "json" || view.kind === "text" || view.kind === "protobuf" || view.kind === "hex") && view.truncated && (
        <p className="kafka-quiet">
          Cut at {view.kind === "hex" ? `${view.size} bytes seen` : "the page's limit"} — the whole value is not sent to this window.
        </p>
      )}
    </div>
  );
}

/**
 * Sending one record: a key, headers and a text value, at a partition or
 * wherever the producer puts it. Nothing here assembles protobuf — the value is
 * UTF-8 text (decision 2), so a payload built by a schema is out of reach.
 */
function SendForm({
  open,
  topic,
  partitionCount,
  onClose,
  onSend,
}: {
  open: boolean;
  topic: string;
  partitionCount: number;
  onClose: () => void;
  onSend: (spec: ProduceSpec) => Promise<string | null>;
}) {
  const [partition, setPartition] = useState("");
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const [headers, setHeaders] = useState<[string, string][]>([]);
  const [fault, setFault] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setPartition("");
    setKey("");
    setValue("");
    setHeaders([]);
    setFault(null);
    setBusy(false);
  }, [open]);

  const send = async () => {
    setBusy(true);
    setFault(null);
    const text = partition.trim();
    if (text !== "" && !/^\d+$/.test(text)) {
      setFault("a partition is a whole number, or nothing");
      setBusy(false);
      return;
    }
    if (text !== "" && Number(text) >= partitionCount) {
      setFault(`this topic has partitions 0–${partitionCount - 1}`);
      setBusy(false);
      return;
    }
    const answer = await onSend({
      topic,
      partition: text === "" ? null : Number(text),
      key: key === "" ? null : key,
      headers: headers.filter(([name]) => name.trim() !== ""),
      value,
    });
    if (answer !== null) setFault(answer);
    setBusy(false);
  };

  return (
    <Modal
      title={`Send to ${topic}`}
      open={open}
      onClose={() => {
        // The answer is on its way: closing now would hide where it landed.
        if (!busy) onClose();
      }}
      footer={
        <>
          <button type="button" className="btn btn-ghost" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void send()}>
            Send
          </button>
        </>
      }
    >
      <div className="kafka-form">
        <label className="kafka-field">
          <span>Partition — empty for the producer's own choice</span>
          <input aria-label="Partition" value={partition} onChange={(event) => setPartition(event.target.value)} />
        </label>
        <label className="kafka-field">
          <span>Key</span>
          <input aria-label="Key" value={key} onChange={(event) => setKey(event.target.value)} />
        </label>

        <div className="kafka-headers">
          <span className="kafka-field-label">Headers</span>
          {headers.map(([name, headerValue], index) => (
            <div key={index} className="kafka-header-row">
              <input
                aria-label={`Header ${index + 1} name`}
                placeholder="name"
                value={name}
                onChange={(event) => setHeaders(headers.map((row, at) => (at === index ? [event.target.value, row[1]] : row)))}
              />
              <input
                aria-label={`Header ${index + 1} value`}
                placeholder="value"
                value={headerValue}
                onChange={(event) => setHeaders(headers.map((row, at) => (at === index ? [row[0], event.target.value] : row)))}
              />
              <button type="button" className="kafka-icon-button" aria-label={`Remove header ${index + 1}`} onClick={() => setHeaders(headers.filter((_, at) => at !== index))}>
                ×
              </button>
            </div>
          ))}
          <button type="button" className="btn btn-ghost" onClick={() => setHeaders([...headers, ["", ""]])}>
            <Plus size={13} /> Add header
          </button>
        </div>

        <label className="kafka-field">
          <span>Value — text, sent as UTF-8</span>
          <textarea aria-label="Value" rows={6} value={value} onChange={(event) => setValue(event.target.value)} />
        </label>

        {fault && <p className="kafka-error" role="alert">{fault}</p>}
        <p className="kafka-quiet">An empty value is a record with nothing in it — not a tombstone, which this cannot write.</p>
      </div>
    </Modal>
  );
}

/** The form: brokers, TLS, a login, and the two switches the whole page obeys. */
type FormProps = {
  draft: Draft;
  /** Why the last save was refused: shown in the form, where Save was pressed. */
  fault?: string | null;
  /** A connection already saved: it can be deleted from its settings. */
  existing: boolean;
  onSave: (connection: Connection) => Promise<boolean>;
  onProbe: (id: string) => Promise<{ ok: boolean; text: string }>;
  onDelete: () => void;
  onClose: () => void;
};

function ConnectionForm({ draft, ...rest }: Omit<FormProps, "draft"> & { draft: Draft | null }) {
  // Keyed by the connection: opening another one starts from its own fields,
  // and Save reads the fields that are on screen rather than the draft it was
  // opened with.
  return draft ? <OpenForm key={draft.id} draft={draft} {...rest} /> : null;
}

function OpenForm({ draft, existing, fault, onSave, onProbe, onDelete, onClose }: FormProps) {
  const [fields, setFields] = useState(draft);
  // What the broker said, under the fields it was asked about — as the
  // MongoDB page's connection form shows its test.
  const [tested, setTested] = useState<{ ok: boolean; text: string } | null>(null);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setFields((current) => ({ ...current, [key]: value }));

  return (
    <Modal
      title="Connection"
      open
      onClose={onClose}
      footer={
        <>
          {existing && (
            <>
              <button type="button" className="btn btn-ghost kafka-delete" onClick={onDelete}>
                Delete…
              </button>
              <button type="button" className="btn btn-ghost" onClick={() => void onProbe(fields.id).then(setTested)}>
                <Check size={13} /> Test
              </button>
            </>
          )}
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={fields.name.trim() === "" || fields.brokers.trim() === ""}
            onClick={() => {
              // Closed on a yes only: a refused save keeps what was typed.
              void onSave(toConnection(fields)).then((saved) => saved && onClose());
            }}
          >
            Save
          </button>
        </>
      }
    >
      <div className="kafka-form">
        <label className="kafka-field">
          <span>Name</span>
          <input aria-label="Name" value={fields.name} onChange={(event) => set("name", event.target.value)} placeholder="Prod" autoFocus />
        </label>
        <label className="kafka-field">
          <span>Brokers</span>
          <input
            className="kafka-mono"
            aria-label="Brokers"
            value={fields.brokers}
            onChange={(event) => set("brokers", event.target.value)}
            placeholder="broker-1:9092,broker-2:9092"
          />
        </label>

        <div className="kafka-check">
          <Switch label="TLS" on={fields.tls} onChange={(on) => set("tls", on)} />
          <span>TLS — the broker is reached over SSL</span>
        </div>
        {fields.tls && (
          <label className="kafka-field kafka-indent">
            <span>CA file, when the cluster has its own (else the bundled roots)</span>
            <input className="kafka-mono" aria-label="CA file" value={fields.caFile} onChange={(event) => set("caFile", event.target.value)} placeholder="/etc/kafka/ca.pem" />
          </label>
        )}

        <div className="kafka-check">
          <Switch label="SASL login" on={fields.login} onChange={(on) => set("login", on)} />
          <span>SASL login</span>
        </div>
        {fields.login && (
          <>
            <label className="kafka-field kafka-indent">
              <span>Mechanism</span>
              <Dropdown
                label={MECHANISMS.find((m) => m.value === fields.mechanism)?.label ?? "Mechanism"}
                title="SASL mechanism"
                value={fields.mechanism}
                options={MECHANISMS}
                onPick={(value) => set("mechanism", value as SaslMechanism)}
                below
              />
            </label>
            <label className="kafka-field kafka-indent">
              <span>Username</span>
              <input aria-label="Username" value={fields.username} onChange={(event) => set("username", event.target.value)} />
            </label>
            <label className="kafka-field kafka-indent">
              <span>Password{fields.hasPassword ? " — saved, leave empty to keep it" : ""}</span>
              <input type="password" aria-label={fields.hasPassword ? "Password, saved — leave empty to keep it" : "Password"} value={fields.password} onChange={(event) => set("password", event.target.value)} autoComplete="new-password" />
            </label>
          </>
        )}

        <div className="kafka-check">
          <Switch label="Read-only" on={fields.readOnly} onChange={(on) => set("readOnly", on)} />
          <span>Read-only — nothing is sent through it</span>
        </div>
        <div className="kafka-check">
          <Switch label="Open to the agent" on={fields.agent} onChange={(on) => set("agent", on)} />
          <span>Open to the agent — its `kafka` tool reads this cluster by the name above</span>
        </div>

        {fault && <p className="kafka-error" role="alert">{fault}</p>}
        {tested && <p className={tested.ok ? "kafka-ok" : "kafka-error"}>{tested.text}</p>}
        <p className="kafka-quiet">Test asks the saved connection — the broker answers about itself.</p>
      </div>
    </Modal>
  );
}

