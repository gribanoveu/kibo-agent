import { ArrowDown } from "lucide-react";
import "./ScrollToBottomButton.css";

/**
 * The way back to the end of a chat's thread, shown once the reader has
 * scrolled away from it. Rendered inside a positioned panel whose last child
 * is the scrolling thread (`.chat-panel`, `.plain-panel`), so it floats over
 * the thread's bottom edge rather than scrolling with its content.
 */
export function ScrollToBottomButton({ atBottom, onClick }: { atBottom: boolean; onClick: () => void }) {
  if (atBottom) return null;
  return (
    <button
      type="button"
      className="scroll-to-bottom"
      title="Back to the end of the conversation"
      onClick={onClick}
    >
      <ArrowDown size={14} />
    </button>
  );
}