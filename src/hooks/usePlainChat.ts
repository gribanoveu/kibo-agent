import { useEffect, useRef, useState } from "react";
import {
  deleteChat,
  loadChat,
  onTurnEvent,
  plainChatCancel,
  plainChatList,
  plainChatRoles,
  plainChatSave,
  plainChatSend,
  setChatArchived,
  type ChatRoleId,
  type ChatRoleView,
  type ChatSummary,
  type LlmMessage,
} from "../lib/chat";
import { useStoredState } from "./useStoredState";

export type PlainMessage = { role: "user" | "assistant"; text: string };

const isId = (value: unknown): value is string | null => value === null || typeof value === "string";

const toWire = (messages: PlainMessage[]): LlmMessage[] => messages.map((m) => ({ role: m.role, content: m.text }));
const fromWire = (messages: LlmMessage[]): PlainMessage[] =>
  messages.flatMap((m) =>
    (m.role === "user" || m.role === "assistant") && m.content ? [{ role: m.role, text: m.content }] : [],
  );

/**
 * Chat mode: its conversations, stored by the backend, and the one open. The
 * model reads the same list the reader does — a plain chat has no tool calls
 * to keep apart from what is shown.
 */
export function usePlainChat() {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  // Which chat is open, so the next launch opens it again. Null until the first message is saved.
  const [chatId, setChatId] = useStoredState<string | null>("plain-chat-open", null, isId);
  const [messages, setMessages] = useState<PlainMessage[]>([]);
  const [roles, setRoles] = useState<ChatRoleView[]>([]);
  const [role, setRole] = useState<ChatRoleId>("assistant");
  // The reply being written; null when none is.
  const [streaming, setStreaming] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Bumped whenever another conversation takes the screen: a reply that lands
  // after it belongs to one that is no longer shown. It is saved all the same.
  const epoch = useRef(0);

  const refresh = () =>
    plainChatList().then(setChats, (e) => setError(String(e)));

  useEffect(() => {
    plainChatRoles().then(setRoles, () => {});
    void refresh();
    if (chatId) void open(chatId);
  }, []);

  const show = (id: string | null, shown: PlainMessage[], shownRole: ChatRoleId) => {
    if (streaming !== null) void plainChatCancel().catch(() => {});
    epoch.current += 1;
    setChatId(id);
    setMessages(shown);
    setRole(shownRole);
    setStreaming(null);
    setError(null);
  };

  const open = async (id: string) => {
    try {
      const record = await loadChat(id);
      show(id, fromWire(record.messages), record.role ?? "assistant");
    } catch (e) {
      // A chat deleted since it was left open is simply not reopened.
      show(null, [], "assistant");
      if (id !== chatId) setError(String(e));
    }
  };

  const newChat = () => show(null, [], role);

  const send = async (text: string) => {
    if (streaming !== null || !text.trim()) return;
    const asked = epoch.current;
    const id = chatId ?? crypto.randomUUID();
    const history: PlainMessage[] = [...messages, { role: "user", text }];
    setChatId(id);
    setMessages(history);
    setStreaming("");
    setError(null);
    const current = () => epoch.current === asked;
    const turnId = crypto.randomUUID();
    const unlisten = await onTurnEvent(turnId, (event) => {
      if (event.type === "delta" && current()) setStreaming((s) => (s ?? "") + event.payload.delta);
    });
    try {
      // Saved before the reply: a reply that fails leaves what the user said to retry, not retype.
      await plainChatSave(id, role, toWire(history));
      void refresh();
      const reply = await plainChatSend(turnId, role, toWire(history));
      if (reply.text) {
        const answered: PlainMessage[] = [...history, { role: "assistant", text: reply.text }];
        await plainChatSave(id, role, toWire(answered));
        if (current()) setMessages(answered);
        void refresh();
      }
    } catch (e) {
      if (current()) setError(String(e));
    } finally {
      unlisten();
      if (current()) setStreaming(null);
    }
  };

  const stop = () => void plainChatCancel().catch(() => {});

  const remove = async (id: string) => {
    try {
      await deleteChat(id);
      if (id === chatId) show(null, [], role);
    } catch (e) {
      setError(String(e));
    }
    void refresh();
  };

  const archive = async (id: string, archived: boolean) => {
    try {
      await setChatArchived(id, archived);
    } catch (e) {
      setError(String(e));
    }
    void refresh();
  };

  return { chats, chatId, messages, roles, role, setRole, streaming, error, send, stop, open, newChat, remove, archive };
}

export type PlainChatState = ReturnType<typeof usePlainChat>;
