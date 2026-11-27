import { KiboBrand } from "./KiboBrand";
import "./ChatEmptyState.css";

/** Fills the thread while there is nothing to show: the wordmark alone, as a
    start screen's is — a new chat is a place, not a list that is empty. The
    folder, open or not, is the composer's business: it offers the picker. */
export function ChatEmptyState() {
  return (
    <div className="chat-empty">
      <KiboBrand />
    </div>
  );
}
