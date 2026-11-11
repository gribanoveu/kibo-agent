import { useCallback, useEffect, useRef, useState } from "react";
import {
  kafkaConnectionCheck,
  kafkaConnectionDelete,
  kafkaConnectionSave,
  kafkaConnections,
  kafkaTopic,
  kafkaTopics,
  type Connection,
  type ConnectionView,
  type TopicDetail,
  type TopicView,
} from "../lib/kafka";

/** How long a keystroke waits before the broker is asked for the list again. */
const SEARCH_PAUSE = 300;

/** A failure, as one sentence: the backend already writes them for a person. */
const said = (error: unknown) => String(error);

/**
 * The Kafka page's state: the connections, the open one, its topics and the
 * one topic opened. No channel: only this page changes what it shows, and each
 * change answers with the list after it or reads again.
 */
export function useKafka() {
  const [connections, setConnections] = useState<ConnectionView[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [topics, setTopics] = useState<TopicView[]>([]);
  const [more, setMore] = useState(0);
  const [search, setSearch] = useState("");
  const [detail, setDetail] = useState<TopicDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState(false);

  const open = connections.find((connection) => connection.id === selected) ?? null;

  // The list, read once: nothing outside this window saves a connection.
  const load = useCallback(async () => {
    try {
      const list = await kafkaConnections();
      setConnections(list);
      setSelected((current) => (current && list.some((c) => c.id === current) ? current : (list[0]?.id ?? null)));
      setError(null);
    } catch (failure) {
      setError(said(failure));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // The topics of the open connection, searched — after the typing stops.
  const wanted = useRef(0);
  useEffect(() => {
    if (!selected) {
      setTopics([]);
      setMore(0);
      return;
    }
    const asked = ++wanted.current;
    const timer = window.setTimeout(() => {
      setReading(true);
      kafkaTopics(selected, search.trim() || null)
        .then((list) => {
          if (asked !== wanted.current) return; // a later search answered already
          setTopics(list.topics);
          setMore(list.more);
          setError(null);
        })
        .catch((failure) => asked === wanted.current && setError(said(failure)))
        .finally(() => asked === wanted.current && setReading(false));
    }, search === "" ? 0 : SEARCH_PAUSE);
    return () => window.clearTimeout(timer);
  }, [selected, search]);

  const save = useCallback(async (connection: Connection) => {
    setBusy(true);
    try {
      const list = await kafkaConnectionSave(connection);
      setConnections(list);
      setSelected(connection.id);
      setDetail(null);
      setError(null);
    } catch (failure) {
      setError(said(failure));
    } finally {
      setBusy(false);
    }
  }, []);

  const remove = useCallback(
    async (id: string) => {
      setBusy(true);
      try {
        const list = await kafkaConnectionDelete(id);
        setConnections(list);
        setSelected((current) => (current === id ? (list[0]?.id ?? null) : current));
        setDetail(null);
        setError(null);
      } catch (failure) {
        setError(said(failure));
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  const check = useCallback(async () => {
    if (!selected) return;
    setBusy(true);
    setNote(null);
    try {
      setNote(await kafkaConnectionCheck(selected));
      setError(null);
    } catch (failure) {
      setError(said(failure));
    } finally {
      setBusy(false);
    }
  }, [selected]);

  const openTopic = useCallback(
    async (name: string) => {
      if (!selected) return;
      setBusy(true);
      try {
        setDetail(await kafkaTopic(selected, name));
        setError(null);
      } catch (failure) {
        setError(said(failure));
      } finally {
        setBusy(false);
      }
    },
    [selected],
  );

  return {
    connections,
    selected,
    open,
    topics,
    more,
    search,
    detail,
    error,
    note,
    busy,
    reading,
    select: (id: string) => {
      setSelected(id);
      setDetail(null);
      setNote(null);
      setError(null);
    },
    setSearch,
    save,
    remove,
    check,
    openTopic,
    closeTopic: () => setDetail(null),
    reload: load,
  };
}
