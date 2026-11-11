import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, Database, MoreHorizontal, Plus, RefreshCw, Search } from "lucide-react";
import "./KafkaPage.css";
import { Modal } from "./Modal";
import { Page } from "./Page";
import { Switch } from "./Switch";
import { Dropdown } from "./Dropdown";
import { useKafka } from "../hooks/useKafka";
import { kafkaMessages, valueText, type Connection, type ConnectionView, type MessageView, type ReadPage, type SaslMechanism, type TopicDetail, type TopicView } from "../lib/kafka";

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

  return (
    <Page title="Kafka">
      <div className="kafka-page">
      <nav className="kafka-side" aria-label="Connections">
        <div className="kafka-side-head">
          <span>Connections</span>
          <button
            type="button"
            className="kafka-icon-button"
            title="New connection"
            aria-label="New connection"
            onClick={() => setEditing(blank())}
          >
            <Plus size={14} />
          </button>
          <button type="button" className="kafka-icon-button" title="Read again" aria-label="Read the connections again" onClick={() => void kafka.reload()}>
            <RefreshCw size={13} />
          </button>
        </div>
        {kafka.connections.length === 0 && <p className="kafka-quiet kafka-side-empty">No connections yet.</p>}
        <ul className="kafka-list">
          {kafka.connections.map((connection) => {
            const on = connection.id === kafka.selected;
            return (
              <li key={connection.id}>
                <button
                  type="button"
                  className={`kafka-connection${on ? " on" : ""}`}
                  aria-current={on ? "true" : undefined}
                  onClick={() => kafka.select(connection.id)}
                >
                  <span className="kafka-connection-name">{connection.name}</span>
                  <span className="kafka-connection-brokers">{connection.brokers}</span>
                  <span className="kafka-tags">
                    {connection.readOnly && <span className="kafka-tag">read-only</span>}
                    {connection.tls && <span className="kafka-tag">TLS</span>}
                    {connection.login && <span className="kafka-tag">{connection.login.mechanism === "plain" ? "PLAIN" : "SCRAM"}</span>}
                    {connection.agent && <span className="kafka-tag kafka-tag-agent">agent</span>}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      </nav>

      <section className="kafka-main" aria-label={kafka.open ? kafka.open.name : "Kafka"}>
        {kafka.error && <p className="kafka-error" role="alert">{kafka.error}</p>}
        {!kafka.open && (
          <p className="kafka-empty">
            Add a connection: the brokers of a cluster, and a login if it wants one. This stage only reads — nothing is ever sent to a cluster.
          </p>
        )}

        {kafka.open && (
          <>
            <header className="kafka-head">
              <div>
                <h2 className="kafka-title">{kafka.open.name}</h2>
                <p className="kafka-sub kafka-mono">{kafka.open.brokers}</p>
              </div>
              <div className="kafka-head-actions">
                <button type="button" className="btn btn-ghost" disabled={kafka.busy} onClick={() => void kafka.check()}>
                  <Check size={13} /> Test
                </button>
                <Dropdown
                  label={<MoreHorizontal size={14} aria-label="Connection actions" />}
                  title="Connection actions"
                  value=""
                  options={[{ value: "delete", label: "Delete connection…", hint: "Its saved password goes with it" }]}
                  onPick={(value) => value === "delete" && setDropping(kafka.open)}
                />
              </div>
            </header>
            <button type="button" className="kafka-edit-link" onClick={() => kafka.open && setEditing(fromSaved(kafka.open))}>
              Edit this connection
            </button>

            {kafka.note && <p className="kafka-ok">{kafka.note}</p>}

            {kafka.detail ? (
              <Topic detail={kafka.detail} connection={kafka.open.id} onBack={kafka.closeTopic} />
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
                  {kafka.reading && <span className="kafka-quiet" role="status">Reading…</span>}
                </div>

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

                {kafka.more > 0 && (
                  <p className="kafka-quiet">
                    {count(kafka.more, "more topic")} — search narrows the list.
                  </p>
                )}
              </>
            )}
          </>
        )}
      </section>

      <ConnectionForm draft={editing} onSave={kafka.save} onClose={() => setEditing(null)} />

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
        <p>
          “{dropping?.name}” and the password saved with it go from this computer. The cluster itself is not touched.
        </p>
      </Modal>
      </div>
    </Page>
  );
}

/** One topic opened: its partitions and the configs a reader cares about. */
function Topic({ detail, connection, onBack }: { detail: TopicDetail; connection: string; onBack: () => void }) {
  const total = detail.topic.partitions.reduce((sum, partition) => sum + Math.max(0, partition.latest - partition.earliest), 0);
  return (
    <div className="kafka-topic-panel">
      <div className="kafka-topic-head">
        <button type="button" className="kafka-icon-button" title="Back to the topics" aria-label="Back to the topics" onClick={onBack}>
          <ArrowLeft size={14} />
        </button>
        <h3 className="kafka-topic-name">{detail.topic.name}</h3>
        <span className="kafka-quiet">
          {count(detail.topic.partitions.length, "partition")}, {count(total, "record")}
        </span>
      </div>

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

      <Messages connection={connection} topic={detail.topic} />

      {detail.configs.length > 0 && (
        <>
          <h4 className="kafka-section-title">Configs</h4>
          <dl className="kafka-configs">
            {detail.configs.map((config) => (
              <div key={config.name} className="kafka-config">
                <dt>{config.name}</dt>
                <dd className="kafka-mono">{config.value || "—"}</dd>
              </div>
            ))}
          </dl>
        </>
      )}
    </div>
  );
}

/** How many messages one answer carries, as the buttons offer them. */
const COUNTS = [20, 50, 100];

/**
 * The topic's messages: which partitions to read, from where and how many, and
 * one message opened — its key, headers and value in whichever view its bytes
 * turned out to be.
 */
function Messages({ connection, topic }: { connection: string; topic: TopicView }) {
  const [partitions, setPartitions] = useState<number[]>([]);
  const [count, setCount] = useState(20);
  const [from, setFrom] = useState("");
  const [page, setPage] = useState<ReadPage | null>(null);
  const [opened, setOpened] = useState<MessageView | null>(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Which read the answer belongs to: a chip clicked while a read is in flight
  // asks a new question, and the older answer must not become the page.
  const asking = useRef(0);
  const read = useCallback(
    async (at: string, howMany: number, chosen: number[]) => {
      const mine = ++asking.current;
      setReading(true);
      setError(null);
      // The old window goes with the old question: a refused read must not leave
      // the last one's rows under its error.
      setPage(null);
      setOpened(null);
      try {
        const text = at.trim();
        if (text !== "" && !/^\d+$/.test(text)) throw new Error("an offset is a whole number, or nothing");
        const offset = text === "" ? null : Number(text);
        const answer = await kafkaMessages(connection, { topic: topic.name, partitions: chosen, from: offset, count: howMany });
        if (mine === asking.current) setPage(answer);
      } catch (failure) {
        if (mine === asking.current) setError(String(failure));
      } finally {
        if (mine === asking.current) setReading(false);
      }
    },
    [connection, topic.name],
  );

  useEffect(() => {
    void read("", count, partitions);
    // A partition picked or a count chosen is a new question; the offset box is
    // read only when it is asked for.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connection, topic.name, count, partitions]);

  const toggle = (partition: number) =>
    setPartitions((current) => (current.includes(partition) ? current.filter((p) => p !== partition) : [...current, partition].sort((a, b) => a - b)));

  return (
    <section className="kafka-messages" aria-label={`Messages of ${topic.name}`}>
      <div className="kafka-message-bar">
        <span className="kafka-section-title">Messages</span>
        <div className="kafka-partitions" role="group" aria-label="Partitions to read">
          <button
            type="button"
            className={`kafka-chip${partitions.length === 0 ? " on" : ""}`}
            aria-pressed={partitions.length === 0}
            onClick={() => setPartitions([])}
          >
            All
          </button>
          {topic.partitions.map((partition) => (
            <button
              key={partition.partition}
              type="button"
              className={`kafka-chip${partitions.includes(partition.partition) ? " on" : ""}`}
              aria-pressed={partitions.includes(partition.partition)}
              onClick={() => toggle(partition.partition)}
            >
              {partition.partition}
            </button>
          ))}
        </div>
        <span className="kafka-spacer" />
        {COUNTS.map((howMany) => (
          <button key={howMany} type="button" className={`kafka-chip${count === howMany ? " on" : ""}`} disabled={reading} onClick={() => setCount(howMany)}>
            Last {howMany}
          </button>
        ))}
        <input
          className="kafka-offset"
          aria-label="Read from offset"
          placeholder="from offset"
          value={from}
          onChange={(event) => setFrom(event.target.value)}
        />
        <button type="button" className="btn btn-ghost" disabled={reading} onClick={() => void read(from, count, partitions)}>
          Read
        </button>
      </div>

      {topic.partitions.length > 20 && partitions.length === 0 && (
        <p className="kafka-quiet">This topic has {topic.partitions.length} partitions and one answer spans 20 — pick the ones to read.</p>
      )}
      {error && <p className="kafka-error" role="alert">{error}</p>}
      {reading && <p className="kafka-quiet" role="status">Reading…</p>}

      {page && !reading && !error && page.messages.length === 0 && <p className="kafka-quiet">Nothing at that offset in the chosen partitions.</p>}

      {page && page.messages.length > 0 && (
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
            {page.messages.map((message) => {
              const on = opened?.partition === message.partition && opened.offset === message.offset;
              return (
                <tr key={`${message.partition}/${message.offset}`} className={on ? "on" : undefined}>
                  <td>{message.partition}</td>
                  <td>
                    <button
                      type="button"
                      className="kafka-topic"
                      aria-label={`Message ${message.partition}/${message.offset}`}
                      onClick={() => setOpened(on ? null : message)}
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
      {page?.more && <p className="kafka-quiet">The count ran out before the log did — read on from the last offset.</p>}

      {opened && <MessageBody message={opened} />}
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

/** The form: brokers, TLS, a login, and the two switches the whole page obeys. */
function ConnectionForm({ draft, onSave, onClose }: { draft: Draft | null; onSave: (connection: Connection) => Promise<boolean>; onClose: () => void }) {
  // Keyed by the connection: opening another one starts from its own fields,
  // and Save reads the fields that are on screen rather than the draft it was
  // opened with.
  return draft ? <OpenForm key={draft.id} draft={draft} onSave={onSave} onClose={onClose} /> : null;
}

function OpenForm({ draft, onSave, onClose }: { draft: Draft; onSave: (connection: Connection) => Promise<boolean>; onClose: () => void }) {
  const [fields, setFields] = useState(draft);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setFields((current) => ({ ...current, [key]: value }));

  return (
    <Modal
      title="Connection"
      open
      onClose={onClose}
      footer={
        <>
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
          <span>Read-only — kept on the connection; whatever writes later refuses one marked so</span>
        </div>
        <div className="kafka-check">
          <Switch label="Open to the agent" on={fields.agent} onChange={(on) => set("agent", on)} />
          <span>Open to the agent — kept on the connection, for the tool that comes with the messages stage</span>
        </div>

        <p className="kafka-quiet">Save it, then Test — the broker is asked about itself once the connection is saved. Reading messages and sending them are the next stages, so the two flags are saved now and obeyed when they arrive.</p>
      </div>
    </Modal>
  );
}

export default KafkaPage;
