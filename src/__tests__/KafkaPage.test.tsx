import { afterEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { KafkaPage } from "../components/KafkaPage";

// What the page sends is what the sealed store reads: camelCase keys — a
// `read_only` saved as false on a production cluster is the failure worth a
// test — no login at all when the login is off, and an empty password when the
// saved one is not shown, which the store keeps.

type Call = { command: string; args: Record<string, unknown> };

const connections = [
  {
    id: "c1",
    name: "prod",
    brokers: "broker-1:9092",
    tls: true,
    caFile: "/etc/kafka/ca.pem",
    login: { mechanism: "scramSha512", username: "reports" },
    hasPassword: true,
    readOnly: false,
    agent: false,
  },
  {
    id: "c2",
    name: "reports",
    brokers: "broker-2:9092",
    tls: false,
    caFile: null,
    login: null,
    hasPassword: false,
    readOnly: true,
    agent: false,
  },
];

const topic = { name: "orders", internal: false, partitions: [{ partition: 0, leader: 1, replicas: [1], inSync: [1], earliest: 0, latest: 3 }] };

let calls: Call[] = [];
/** A store that will not open, and a cluster that will not answer. */
let refuseList = false;
let refuseTopics = false;
let refuseSave = false;
const liveChannels: { onmessage: ((batch: unknown) => void) | null }[] = [];
let refuseProduce = false;
let producePartition = 0;
/** Answers held back, so two reads can come home out of order. */
let holdMessages = false;
const held: ((answer: unknown) => void)[] = [];

mock.module("@tauri-apps/api/core", () => ({
  /** The wrapper hands a channel to the command; the test pushes a tick into it. */
  Channel: class {
    onmessage: ((batch: unknown) => void) | null = null;
    constructor() {
      liveChannels.push(this);
    }
  },
  invoke: (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    switch (command) {
      case "kafka_connections":
        return refuseList ? Promise.reject("could not read kafka_connections.enc") : Promise.resolve(connections);
      case "kafka_connection_save":
        return refuseSave ? Promise.reject("kafka_connections.enc could not be sealed") : Promise.resolve(connections);
      case "kafka_groups":
        return Promise.resolve({
          total: 21,
          unreadBrokers: 1,
          groups: [
            { name: "probe-group", state: "Stable", members: 2, lagsRead: true, totalLag: 12, lags: [
              { group: "probe-group", topic: "orders", partition: 0, committed: 88, latest: 100, lag: 12, active: false },
              { group: "probe-group", topic: "orders", partition: 1, committed: null, latest: 40, lag: null, active: false },
            ] },
            { name: "late-group", state: "Empty", members: 0, lagsRead: false, totalLag: null, lags: [] },
          ],
        });
      case "kafka_live_start":
        return Promise.resolve("live-1");
      case "kafka_live_stop":
        return Promise.resolve(null);
      case "kafka_connection_delete":
        return Promise.resolve([]);
      case "kafka_topics":
        return refuseTopics ? Promise.reject("the topics: no answer in 15s") : Promise.resolve({ topics: [topic], total: 240 });
      case "kafka_topic":
        return Promise.resolve({ topic, configs: [{ name: "cleanup.policy", value: "delete" }] });
      case "kafka_produce":
        if (refuseProduce) return Promise.reject("the send: Topic orders not found");
        return Promise.resolve({ partition: producePartition, offset: 812 });
      case "kafka_messages":
        if (holdMessages) return new Promise((resolve) => held.push(resolve));
        return Promise.resolve({
          partitions: [0],
          more: true,
          messages: [
            {
              partition: 0,
              offset: 2,
              timestamp: 1_700_000_000_000,
              key: { kind: "text", text: "A-3", truncated: false },
              value: { kind: "protobuf", text: '1: "PAY-1"\n5: 30', schemaId: null, truncated: false },
              size: 21,
              headers: [{ name: "source", value: "api" }],
            },
            {
              partition: 0,
              offset: 3,
              timestamp: 1_700_000_001_000,
              key: null,
              value: { kind: "text", text: "cut", truncated: true },
              size: 70_000,
              headers: [],
            },
          ],
        });
      default:
        return Promise.resolve(null);
    }
  },
  transformCallback: (callback: unknown) => callback,
}));

afterEach(() => {
  calls = [];
  refuseList = false;
  refuseTopics = false;
  refuseSave = false;
  refuseProduce = false;
  producePartition = 0;
  liveChannels.length = 0;
  holdMessages = false;
  held.length = 0;
});

const sent = (command: string) => calls.filter((call) => call.command === command);

describe("the Kafka page", () => {
  test("lists the saved connections and the topics of the open one", async () => {
    render(<KafkaPage />);
    expect(await screen.findByRole("heading", { name: "prod" })).toBeTruthy();
    expect(await screen.findByText("orders")).toBeTruthy();
    expect(sent("kafka_topics")[0].args).toMatchObject({ connection: "c1", name: null, page: 0 });
  });

  test("a new connection is saved with the window's own key names", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByRole("button", { name: "Connection" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: " stage " } });
    fireEvent.change(screen.getByLabelText("Brokers"), { target: { value: " stage-1:9092 " } });
    fireEvent.click(screen.getByLabelText("Read-only"));
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => expect(sent("kafka_connection_save")).toHaveLength(1));
    const connection = sent("kafka_connection_save")[0].args.connection as Record<string, unknown>;
    expect(connection.name).toBe("stage");
    expect(connection.brokers).toBe("stage-1:9092");
    expect(connection.readOnly).toBe(true);
    expect(connection.tls).toBe(false);
    expect(connection.caFile).toBeNull();
    expect(connection.sasl).toBeNull();
    expect(connection.id).toEqual(expect.any(String));
  });

  test("a store that will not open says so, rather than reading as no connections", async () => {
    refuseList = true;
    render(<KafkaPage />);
    const shown = await screen.findByRole("alert");
    expect(shown.textContent).toContain("could not read");
  });

  test("a save that is refused keeps the dialog and what was typed in it", async () => {
    refuseSave = true;
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByRole("button", { name: "Connection" }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "stage" } });
    fireEvent.change(screen.getByLabelText("Brokers"), { target: { value: "stage-1:9092" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toBeTruthy();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("stage");
    expect((screen.getByLabelText("Brokers") as HTMLInputElement).value).toBe("stage-1:9092");
  });

  test("a connection without a name or brokers cannot be saved", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByRole("button", { name: "Connection" }));
    const save = () => screen.getByRole("button", { name: "Save" }) as HTMLButtonElement;
    expect(save().disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "stage" } });
    expect(save().disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Brokers"), { target: { value: "stage-1:9092" } });
    expect(save().disabled).toBe(false);
  });

  test("a topic's messages are listed, and one opens as the bytes it turned out to be", async () => {
    render(<KafkaPage />);
    await screen.findByText("orders");
    fireEvent.click(screen.getByText("orders"));
    // The messages are a tab of the topic, beside its partitions and configs.
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));

    expect(await screen.findByText("A-3")).toBeTruthy();
    expect(sent("kafka_messages")[0].args.spec).toEqual({ topic: "orders", partitions: [], from: null, count: 20 });

    fireEvent.click(screen.getByLabelText("Message 0/2"));
    const value = await screen.findByText(/PAY-1/);
    expect(value.tagName).toBe("PRE");
    expect(screen.getByText("protobuf")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Message 0/3"));
    expect(await screen.findByText(/the whole value is not sent/)).toBeTruthy();
  });

  test("an offset that is not a number is refused before the broker is asked", async () => {
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));
    await screen.findByLabelText("Read from offset");
    const before = sent("kafka_messages").length;

    fireEvent.change(screen.getByLabelText("Read from offset"), { target: { value: "-1" } });
    fireEvent.click(screen.getByRole("button", { name: "Read" }));

    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(sent("kafka_messages")).toHaveLength(before);
  });

  test("a read asked for last does not lose to one asked for before it", async () => {
    holdMessages = true;
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));
    await screen.findByLabelText("Read from offset");
    // The first read (all partitions, the last 20) is still out.
    fireEvent.click(screen.getByRole("button", { name: "0" }));

    const page = (offset: number) => ({
      partitions: [0],
      more: false,
      messages: [{ partition: 0, offset, timestamp: 1, key: null, value: { kind: "text", text: `v${offset}`, truncated: false }, size: 2, headers: [] }],
    });
    await act(async () => held[1](page(2)));
    expect(await screen.findByLabelText("Message 0/2")).toBeTruthy();
    // The older answer arrives last and must not replace it.
    await act(async () => held[0](page(1)));
    expect(screen.queryByLabelText("Message 0/1")).toBeNull();
    expect(screen.getByLabelText("Message 0/2")).toBeTruthy();
  });

  test("the topics turn a page at a time, and the buttons say where they are", async () => {
    render(<KafkaPage />);
    await screen.findByText("orders");
    expect(screen.getByText(/240 topics — 1–1 shown/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "‹ Previous" }) as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Next ›" }));
    await waitFor(() => expect(sent("kafka_topics").some((call) => call.args.page === 1)).toBe(true));

    // Another search is another list: its first page, not the one it was on.
    fireEvent.change(screen.getByLabelText("Find a topic"), { target: { value: "ord" } });
    await waitFor(() => expect(sent("kafka_topics").some((call) => call.args.page === 0 && call.args.name === "ord")).toBe(true));
  });

  test("the refresh button asks the broker again, not the kept shape", async () => {
    render(<KafkaPage />);
    await screen.findByText("orders");
    expect(sent("kafka_topics")[0].args.refresh).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "Read the topics again" }));
    await waitFor(() => expect(sent("kafka_topics").some((call) => call.args.refresh === true)).toBe(true));
  });

  test("a message is sent from the page as the window's own keys, and it says where it landed", async () => {
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));

    fireEvent.click(await screen.findByRole("button", { name: /Send…/ }));
    fireEvent.change(screen.getByLabelText("Partition"), { target: { value: "0" } });
    fireEvent.change(screen.getByLabelText("Key"), { target: { value: "K-1" } });
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "hello" } });
    fireEvent.click(screen.getByRole("button", { name: "Add header" }));
    fireEvent.change(screen.getByLabelText("Header 1 name"), { target: { value: "source" } });
    fireEvent.change(screen.getByLabelText("Header 1 value"), { target: { value: "page" } });
    // A row left with no name is not a header, and is not sent.
    fireEvent.click(screen.getByRole("button", { name: "Add header" }));
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(sent("kafka_produce")).toHaveLength(1));
    expect(sent("kafka_produce")[0].args).toEqual({
      connection: "c1",
      spec: { topic: "orders", partition: 0, key: "K-1", headers: [["source", "page"]], value: "hello" },
    });
    expect(await screen.findByText(/Sent to partition 0 at offset 812/)).toBeTruthy();
    // The dialog is done, and the window it wrote into was read again.
    expect(screen.queryByLabelText("Value")).toBeNull();
    await waitFor(() => expect(sent("kafka_messages").length).toBeGreaterThan(1));
  });

  test("an empty value is a record, and a refused send keeps the dialog with the broker's words", async () => {
    refuseProduce = true;
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));

    fireEvent.click(await screen.findByRole("button", { name: /Send…/ }));
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    expect(await screen.findByText(/Topic orders not found/)).toBeTruthy();
    expect(sent("kafka_produce")[0].args.spec).toEqual({ topic: "orders", partition: null, key: null, headers: [], value: "" });
    expect((screen.getByLabelText("Value") as HTMLTextAreaElement).value).toBe("", "the dialog is still there");
  });

  test("a read-only connection offers no send", async () => {
    render(<KafkaPage />);
    // The connection is chosen from the bar: the trigger says whose brokers it is.
    // The heading carries the same title, so the bar's trigger is the button.
    const trigger = (await screen.findAllByTitle("broker-1:9092")).find((element) => element.tagName === "BUTTON");
    fireEvent.click(trigger as HTMLButtonElement);
    fireEvent.click(await screen.findByText("reports"));
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));

    const send = (await screen.findByRole("button", { name: /Send…/ })) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    // Said where it is read, not only in a tooltip a disabled button never shows.
    expect(screen.getByText(/This connection is read-only/)).toBeTruthy();
  });

  test("a partition that is not a whole number never reaches the broker", async () => {
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));

    fireEvent.click(await screen.findByRole("button", { name: /Send…/ }));
    fireEvent.change(screen.getByLabelText("Partition"), { target: { value: "-2" } });
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "v" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    expect(await screen.findByText(/a partition is a whole number/)).toBeTruthy();
    expect(sent("kafka_produce")).toHaveLength(0);
  });

  test("a live read is started for the chosen partitions and stopped by the toggle", async () => {
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));

    fireEvent.click(await screen.findByRole("button", { name: "Live" }));
    await waitFor(() => expect(sent("kafka_live_start")).toHaveLength(1));
    expect(sent("kafka_live_start")[0].args).toMatchObject({ connection: "c1", spec: { topic: "orders", partitions: [], from: null } });

    fireEvent.click(screen.getByRole("button", { name: "Live" }));
    await waitFor(() => expect(sent("kafka_live_stop")).toHaveLength(1));
    expect(sent("kafka_live_stop")[0].args.id).toBe("live-1");
  });

  test("a tick lands in the table, and a read that ends says why and lets go", async () => {
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Live" }));
    await waitFor(() => expect(liveChannels).toHaveLength(1));

    const tick = {
      more: 3,
      failed: null,
      messages: [{ partition: 0, offset: 40, timestamp: 1, key: null, value: { kind: "text", text: "live", truncated: false }, size: 4, headers: [] }],
    };
    act(() => liveChannels[0].onmessage?.(tick));
    expect(await screen.findByLabelText("Message 0/40")).toBeTruthy();
    expect(screen.getByText(/3 more arrived/)).toBeTruthy();

    act(() => liveChannels[0].onmessage?.({ messages: [], more: 0, failed: "the live read: broker went quiet" }));
    expect(await screen.findByText(/broker went quiet/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Live" }) as HTMLButtonElement).getAttribute("aria-pressed")).toBe("false");
  });

  test("a cluster's groups show their lag, and say when it was not read", async () => {
    render(<KafkaPage />);
    await screen.findByText("orders");

    expect(await screen.findByText("probe-group")).toBeTruthy();
    expect(sent("kafka_groups")[0].args.connection).toBe("c1");
    expect(screen.getByText(/1 broker did not answer/)).toBeTruthy();
    expect(screen.getByText("not read")).toBeTruthy();

    fireEvent.click(screen.getByText("probe-group"));
    // The opened group: the committed end of a partition, and the one without
    // a commit shown as such rather than as zero.
    expect(await screen.findByText("88")).toBeTruthy();
    expect(screen.getByText("no commit")).toBeTruthy();
  });

  test("the refresh asks the broker once, and the next read keeps the cache", async () => {
    render(<KafkaPage />);
    await screen.findByText("orders");

    fireEvent.click(screen.getByRole("button", { name: "Read the topics again" }));
    await waitFor(() => expect(sent("kafka_topics").some((call) => call.args.refresh === true)).toBe(true));

    fireEvent.change(screen.getByLabelText("Find a topic"), { target: { value: "or" } });
    await waitFor(() => expect(sent("kafka_topics").some((call) => call.args.name === "or" && call.args.refresh === false)).toBe(true));
  });

  test("saving an edited connection asks the cluster again", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });
    const before = sent("kafka_topics").length;

    fireEvent.click(screen.getByTitle("Change this connection"));
    fireEvent.change(screen.getByLabelText("Brokers"), { target: { value: "broker-9:9092" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    // Other brokers are another cluster: the list on screen is not its.
    await waitFor(() => expect(sent("kafka_topics").length).toBeGreaterThan(before));
  });

  test("a record sent where this window cannot show it says so", async () => {
    producePartition = 2;
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));
    // Reading partition 0 only: a record that lands in another is not on screen.
    fireEvent.click(await screen.findByRole("button", { name: "0", exact: true }));

    fireEvent.click(await screen.findByRole("button", { name: /Send/ }));
    fireEvent.change(screen.getByLabelText("Value"), { target: { value: "hello" } });
    fireEvent.click(screen.getByRole("button", { name: "Send", exact: true }));

    expect(await screen.findByText(/not in this window/)).toBeTruthy();
  });

  test("leaving the topic ends its live read", async () => {
    const { unmount } = render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Live" }));
    await waitFor(() => expect(sent("kafka_live_start")).toHaveLength(1));

    unmount();
    await waitFor(() => expect(sent("kafka_live_stop")).toHaveLength(1));
    expect(sent("kafka_live_stop")[0].args.id).toBe("live-1");
  });

  test("a long live read keeps the newest, not the first", async () => {
    render(<KafkaPage />);
    fireEvent.click(await screen.findByText("orders"));
    fireEvent.click(await screen.findByRole("tab", { name: /Messages/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Live" }));
    await waitFor(() => expect(liveChannels).toHaveLength(1));

    // Three ticks of 250: more than the window keeps, so what fell off the top
    // is what a reader must not see.
    for (const base of [0, 300]) {
      act(() =>
        liveChannels[0].onmessage?.({
          more: 0,
          failed: null,
          messages: Array.from({ length: 300 }, (_, at) => ({
            partition: 0,
            offset: base + at,
            timestamp: 1,
            key: null,
            value: { kind: "text", text: `t${base + at}`, truncated: false },
            size: 4,
            headers: [],
          })),
        }),
      );
    }

    // The window keeps the newest: what a tick pushed out is gone, and the last
    // one is on screen.
    expect(screen.queryByLabelText("Message 0/0")).toBeNull();
    expect(screen.getByLabelText("Message 0/599")).toBeTruthy();

  });

  test("a broker that answers nothing is an error, not an empty cluster", async () => {
    refuseTopics = true;
    render(<KafkaPage />);
    expect(await screen.findByRole("alert")).toBeTruthy();
    await waitFor(() => expect(screen.queryByText("This cluster has no topics.")).toBeNull());
  });

  test("a TLS connection switched off sends no CA, even one still typed", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByTitle("Change this connection"));
    fireEvent.click(screen.getByLabelText("TLS"));
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() => expect(sent("kafka_connection_save")).toHaveLength(1));
    const connection = sent("kafka_connection_save")[0].args.connection as Record<string, unknown>;
    expect(connection.tls).toBe(false);
    expect(connection.caFile).toBeNull();
  });

  test("an edit sends the saved login with an empty password, and a TLS connection without CA as null", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByTitle("Change this connection"));
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("prod");
    fireEvent.change(screen.getByLabelText("CA file"), { target: { value: "  " } });
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => expect(sent("kafka_connection_save")).toHaveLength(1));
    const connection = sent("kafka_connection_save")[0].args.connection as Record<string, unknown>;
    expect(connection.readOnly).toBe(false);
    expect(connection.caFile).toBeNull();
    expect(connection.sasl).toEqual({ mechanism: "scramSha512", username: "reports", password: "" });
  });
});
