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
 *
 * Every answer says which request it answers, and an answer to a request the
 * page has moved past is dropped: a broker takes seconds, and the last
 * connection's topic list under the new connection's name is a page that lies.
 */
export function useKafka() {
  const [connections, setConnections] = useState<ConnectionView[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [topics, setTopics] = useState<TopicView[]>([]);
  const [total, setTotal] = useState(0);
  /** Bumped by the refresh button: the same question, asked again of the broker. */
  const [again, setAgain] = useState(0);
  const [pageAt, setPageAt] = useState(0);
  const [search, setSearch] = useState("");
  const [detail, setDetail] = useState<TopicDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState(false);

  const open = connections.find((connection) => connection.id === selected) ?? null;

  // Which connection the answers from now on are for, and which topic request
  // the panel is waiting for.
  const looking = useRef<string | null>(null);
  looking.current = selected;
  const opening = useRef(0);

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
  const asked = useRef(0);
  useEffect(() => {
    const mine = ++asked.current;
    // Another connection's topics are not this one's: they go as soon as the
    // connection does, not when its successor answers.
    setTopics([]);
    setTotal(0);
    setReading(false);
    // The counter is bumped above, so an answer to the connection that just
    // went is dropped even when nothing takes its place.
    if (!selected) return;

    const timer = window.setTimeout(
      () => {
        setReading(true);
        kafkaTopics(selected, search.trim() || null, pageAt, again > 0)
          .then((list) => {
            if (mine !== asked.current) return;
            setTopics(list.topics);
            setTotal(list.total);
            setError(null);
          })
          .catch((failure) => mine === asked.current && setError(said(failure)))
          .finally(() => mine === asked.current && setReading(false));
      },
      search === "" ? 0 : SEARCH_PAUSE,
    );
    return () => window.clearTimeout(timer);
  }, [selected, search, pageAt, again]);

  /** Saves, and says whether it landed: the form closes only on a yes. */
  const save = useCallback(async (connection: Connection): Promise<boolean> => {
    setBusy(true);
    try {
      const list = await kafkaConnectionSave(connection);
      setConnections(list);
      setSelected(connection.id);
      setDetail(null);
      setError(null);
      return true;
    } catch (failure) {
      setError(said(failure));
      return false;
    } finally {
      setBusy(false);
    }
  }, []);

  const remove = useCallback(async (id: string) => {
    setBusy(true);
    try {
      const list = await kafkaConnectionDelete(id);
      setConnections(list);
      setSelected((current) => (current === id ? (list[0]?.id ?? null) : current));
      // A topic of the connection that went is not the next one's.
      setDetail((shown) => (id && list.some((c) => c.id === id) ? shown : null));
      setError(null);
    } catch (failure) {
      setError(said(failure));
    } finally {
      setBusy(false);
    }
  }, []);

  /**
   * Asks the broker about a saved connection and hands the answer back: it is
   * the form that shows it, under the fields it belongs to.
   */
  const probe = useCallback(async (id: string): Promise<{ ok: boolean; text: string }> => {
    try {
      return { ok: true, text: await kafkaConnectionCheck(id) };
    } catch (failure) {
      return { ok: false, text: said(failure) };
    }
  }, []);

  const openTopic = useCallback(
    async (name: string) => {
      const forConnection = selected;
      if (!forConnection) return;
      const mine = ++opening.current;
      setBusy(true);
      try {
        const answer = await kafkaTopic(forConnection, name);
        if (mine !== opening.current || forConnection !== looking.current) return;
        setDetail(answer);
        setError(null);
      } catch (failure) {
        if (mine === opening.current && forConnection === looking.current) setError(said(failure));
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
    total,
    pageAt,
    search,
    detail,
    error,
    busy,
    reading,
    select: (id: string) => {
      setSelected(id);
      setSearch("");
      setPageAt(0);
      setDetail(null);
      setError(null);
    },
    setSearch: (text: string) => {
      setSearch(text);
      // Another search is another list: it starts at its first page.
      setPageAt(0);
    },
    setPage: setPageAt,
    refresh: () => setAgain((asked) => asked + 1),
    save,
    remove,
    probe,
    openTopic,
    closeTopic: () => {
      // Back means back: an answer still on its way must not reopen the panel.
      opening.current += 1;
      setDetail(null);
    },
    reload: load,
  };
}
