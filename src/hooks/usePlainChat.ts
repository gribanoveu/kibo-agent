import { useEffect, useRef, useState } from "react";
import { onTurnEvent, plainChatCancel, plainChatRoles, plainChatSend, type ChatRoleId, type ChatRoleView } from "../lib/chat";
import { useStoredState } from "./useStoredState";

export type PlainMessage = { role: "user" | "assistant"; text: string };

const isMessages = (value: unknown): value is PlainMessage[] =>
  Array.isArray(value) &&
  value.every((m) => (m?.role === "user" || m?.role === "assistant") && typeof m?.text === "string");

/**
 * Chat mode: one conversation with the model, kept in this window's storage.
 * The model reads the same list the reader does — a plain chat has no tool
 * calls to keep apart from what is shown.
 */
export function usePlainChat() {
  // ponytail: one conversation in localStorage; a list of chats goes to the backend's store when it is wanted.
  const [messages, setMessages] = useStoredState<PlainMessage[]>("plain-chat", [], isMessages);
  const [roles, setRoles] = useState<ChatRoleView[]>([]);
  const [role, setRole] = useState<ChatRoleId>("assistant");
  // The reply being written; null when none is.
  const [streaming, setStreaming] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Bumped by clear: a reply that lands after it belongs to a conversation that is gone.
  const epoch = useRef(0);

  useEffect(() => {
    plainChatRoles().then(setRoles, () => {});
  }, []);

  const send = async (text: string) => {
    if (streaming !== null || !text.trim()) return;
    const asked = epoch.current;
    const history: PlainMessage[] = [...messages, { role: "user", text }];
    setMessages(history);
    setStreaming("");
    setError(null);
    const turnId = crypto.randomUUID();
    const unlisten = await onTurnEvent(turnId, (event) => {
      if (event.type === "delta" && epoch.current === asked) setStreaming((s) => (s ?? "") + event.payload.delta);
    });
    try {
      const reply = await plainChatSend(
        turnId,
        role,
        history.map((m) => ({ role: m.role, content: m.text })),
      );
      if (epoch.current === asked && reply.text) setMessages([...history, { role: "assistant", text: reply.text }]);
    } catch (e) {
      if (epoch.current === asked) setError(String(e));
    } finally {
      unlisten();
      if (epoch.current === asked) setStreaming(null);
    }
  };

  const stop = () => void plainChatCancel().catch(() => {});

  const clear = () => {
    if (streaming !== null) stop();
    epoch.current += 1;
    setMessages([]);
    setStreaming(null);
    setError(null);
  };

  return { messages, roles, role, setRole, streaming, error, send, stop, clear };
}

export type PlainChatState = ReturnType<typeof usePlainChat>;
