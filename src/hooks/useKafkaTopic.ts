import { useCallback, useEffect, useRef, useState } from "react";
import { kafkaLiveStart, kafkaLiveStop, kafkaMessages, kafkaProduce, type LiveBatch, type MessageView, type ProduceSpec, type ReadPage, type TopicView } from "../lib/kafka";

/** How many messages one answer carries, as the buttons offer them. */
export const COUNTS = [20, 50, 100];
/** `MAX_READ_PARTITIONS` in `domain::kafka`: what one live read follows. */
export const LIVE_PARTITIONS = 20;
/** The window keeps this many messages of a live read; a tick appends to it. */
const KEPT = 500;

/**
 * One topic's messages as the application sees them: which partitions to read,
 * from where and how many, the answer on screen, the live read of it, and the
 * one record sent into it. The component below draws what this says and calls
 * nothing of its own.
 */
export function useKafkaTopic({ connection, topic }: { connection: string; topic: TopicView }) {
  const [partitions, setPartitions] = useState<number[]>([]);
  const [count, setCount] = useState(20);
  const [from, setFrom] = useState("");
  const [page, setPage] = useState<ReadPage | null>(null);
  const [opened, setOpened] = useState<MessageView | null>(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [writing, setWriting] = useState(false);
  const [sent, setSent] = useState<string | null>(null);
  const [living, setLiving] = useState(false);
  const [starting, setStarting] = useState(false);
  const [waiting, setWaiting] = useState(0);
  const liveId = useRef<string | null>(null);
  const gone = useRef(false);

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
    // Another window is another answer: what the last send said was about the
    // one being replaced.
    setSent(null);
    void read("", count, partitions);
    // A partition picked or a count chosen is a new question; the offset box is
    // read only when it is asked for.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connection, topic.name, count, partitions]);

  // A live read ends with the tab: nothing is left looking at a cluster the
  // window no longer shows.
  useEffect(
    () => () => {
      gone.current = true;
      if (liveId.current) void kafkaLiveStop(liveId.current);
    },
    [],
  );

  const toggleLive = async () => {
    if (living) {
      const id = liveId.current;
      liveId.current = null;
      setLiving(false);
      if (id) await kafkaLiveStop(id);
      return;
    }
    setSent(null);
    setError(null);
    setStarting(true);
    try {
      const id = await kafkaLiveStart(connection, { topic: topic.name, partitions, from: null }, (batch: LiveBatch) => {
        // The read says its own end, on the channel it has been reporting to.
        if (batch.failed) {
          liveId.current = null;
          setLiving(false);
          setError(batch.failed);
          return;
        }
        // A tick appends to what is on screen: the window keeps the last few
        // hundred, and says how much is still on its way.
        setWaiting(batch.more);
        setPage((current) => ({
          messages: [...(current?.messages ?? []), ...batch.messages].slice(-KEPT),
          partitions: current?.partitions ?? partitions,
          more: false,
        }));
      });
      if (gone.current) {
        // The tab went while the start was in flight: nothing may be left
        // reading a cluster this window no longer shows.
        void kafkaLiveStop(id);
        return;
      }
      liveId.current = id;
      setLiving(true);
    } catch (failure) {
      setError(String(failure));
    } finally {
      setStarting(false);
    }
  };

  /** Sends one record, and answers what the form should say instead of a fault. */
  const send = async (spec: ProduceSpec): Promise<string | null> => {
    try {
      const written = await kafkaProduce(connection, spec);
      setWriting(false);
      // A record goes where it went: the window on screen may be another
      // partition's, and saying "sent" over a list that cannot hold it reads as
      // a lie.
      const seen = partitions.length === 0 || partitions.includes(written.partition);
      setSent(
        seen
          ? `Sent to partition ${written.partition} at offset ${written.offset}.`
          : `Sent to partition ${written.partition} at offset ${written.offset} — not in this window; read that partition to see it.`,
      );
      void read("", count, partitions);
      return null;
    } catch (failure) {
      return String(failure);
    }
  };

  const togglePartition = (partition: number) =>
    setPartitions((current) => (current.includes(partition) ? current.filter((p) => p !== partition) : [...current, partition].sort((a, b) => a - b)));

  return {
    partitions,
    togglePartition,
    allPartitions: () => setPartitions([]),
    count,
    setCount,
    from,
    setFrom,
    /** The offset box, asked for: the only read that takes what was typed. */
    ask: () => {
      setSent(null);
      return read(from, count, partitions);
    },
    page,
    opened,
    open: (message: MessageView | null) => setOpened(message),
    reading,
    error,
    writing,
    openSend: () => setWriting(true),
    closeSend: () => setWriting(false),
    sent,
    living,
    starting,
    waiting,
    toggleLive,
    send,
  };
}
