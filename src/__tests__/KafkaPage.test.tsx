import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
];

const topic = { name: "orders", internal: false, partitions: [{ partition: 0, leader: 1, replicas: [1], inSync: [1], earliest: 0, latest: 3 }] };

let calls: Call[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    switch (command) {
      case "kafka_connections":

      case "kafka_connection_save":
        return Promise.resolve(connections);
      case "kafka_topics":
        return Promise.resolve({ topics: [topic], more: 0 });
      case "kafka_topic":
        return Promise.resolve({ topic, configs: [{ name: "cleanup.policy", value: "delete" }] });
      default:
        return Promise.resolve(null);
    }
  },
  transformCallback: (callback: unknown) => callback,
}));

afterEach(() => {
  calls = [];
});

const sent = (command: string) => calls.filter((call) => call.command === command);

describe("the Kafka page", () => {
  test("lists the saved connections and the topics of the open one", async () => {
    render(<KafkaPage />);
    expect(await screen.findByRole("heading", { name: "prod" })).toBeTruthy();
    expect(await screen.findByText("orders")).toBeTruthy();
    expect(sent("kafka_topics")[0].args.connection).toBe("c1");
  });

  test("a new connection is saved with the window's own key names", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByLabelText("New connection"));
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

  test("an edit sends the saved login with an empty password, and a TLS connection without CA as null", async () => {
    render(<KafkaPage />);
    await screen.findByRole("heading", { name: "prod" });

    fireEvent.click(screen.getByText("Edit this connection"));
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
