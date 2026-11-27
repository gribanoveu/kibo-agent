import { useMemo } from "react";
import { KiboBrand } from "./KiboBrand";
import { greetingsAt, pickSuggestions } from "../lib/chatSuggestions";
import "./ChatEmptyState.css";

type Props = {
  /** The open chat: the greeting is picked once per chat, not per render. */
  chatId?: string | null;
};

/** Fills the thread while there is nothing to show: the wordmark and the
    hour's greeting, as a start screen's is — a new chat is a place, not a list
    that is empty. The folder, open or not, is the composer's business. */
export function ChatEmptyState({ chatId = null }: Props) {
  // A greeting per empty chat, not per render: it must not change while the user types.
  const greeting = useMemo(() => pickSuggestions(greetingsAt(new Date().getHours()), 1)[0], [chatId]);
  return (
    <div className="chat-empty">
      <KiboBrand />
      <h2>{greeting}</h2>
    </div>
  );
}
