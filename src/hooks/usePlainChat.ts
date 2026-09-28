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

/** One bubble. `reasoning` is the model's thinking before an answer: drawn, never sent back to it. */
export type PlainMessage = { role: "user" | "assistant"; text: string; reasoning?: string };
/** The reply being written: its thinking, then its text. */
export type PlainReply = { text: string; reasoning: string };

const isId = (value: unknown): value is string | null => value === null || typeof value === "string";

const toWire = (messages: PlainMessage[]): LlmMessage[] => messages.map((m) => ({ role: m.role, content: m.text }));
const fromWire = (messages: LlmMessage[]): PlainMessage[] =>
  messages.flatMap((m) =>
    (m.role === "user" || m.role === "assistant") && m.content ? [{ role: m.role, text: m.content }] : [],
  );
const isShown = (value: unknown): value is PlainMessage[] =>
  Array.isArray(value) &&
  value.length > 0 &&
  value.every(
    (m) =>
      (m?.role === "user" || m?.role === "assistant") &&
      typeof m?.text === "string" &&
      (m.reasoning === undefined || typeof m.reasoning === "string"),
  );

/**
 * Chat mode: its conversations, stored by the backend, and the one open. The
 * model is sent the same list the reader sees, less its own thinking.
 */
export function usePlainChat() {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  // Which chat is open, so the next launch opens it again. Null until the first message is saved.
  const [chatId, setChatId] = useStoredState<string | null>("plain-chat-open", null, isId);
  const [messages, setMessages] = useState<PlainMessage[]>([]);
  const [roles, setRoles] = useState<ChatRoleView[]>([]);
  const [role, setRole] = useState<ChatRoleId>("assistant");
  // The reply being written; null when none is.
  const [streaming, setStreaming] = useState<PlainReply | null>(null);
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
      // A chat saved before its transcript was kept is drawn from what the model was sent.
      show(id, isShown(record.blocks) ? record.blocks : fromWire(record.messages), record.role ?? "assistant");
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
    setStreaming({ text: "", reasoning: "" });
    setError(null);
    const current = () => epoch.current === asked;
    const turnId = crypto.randomUUID();
    const unlisten = await onTurnEvent(turnId, (event) => {
      if (!current()) return;
      if (event.type === "delta") setStreaming((s) => s && { ...s, text: s.text + event.payload.delta });
      if (event.type === "reasoning") setStreaming((s) => s && { ...s, reasoning: s.reasoning + event.payload.delta });
    });
    try {
      // Saved before the reply: a reply that fails leaves what the user said to retry, not retype.
      await plainChatSave(id, role, toWire(history), history);
      void refresh();
      const reply = await plainChatSend(turnId, role, toWire(history));
      if (reply.text) {
        const answer: PlainMessage = { role: "assistant", text: reply.text };
        if (reply.reasoning) answer.reasoning = reply.reasoning;
        const answered = [...history, answer];
        await plainChatSave(id, role, toWire(answered), answered);
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
