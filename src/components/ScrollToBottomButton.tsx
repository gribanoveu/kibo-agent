import { useSyncExternalStore } from "react";
import { ArrowDown } from "lucide-react";
import type { FollowBottom } from "../hooks/useFollowBottom";
import "./ScrollToBottomButton.css";

/**
 * The way back to the end of a chat's thread, shown once the reader has
 * scrolled away from it. Rendered inside a positioned panel whose last child
 * is the scrolling thread (`.chat-panel`, `.plain-panel`), so it floats over
 * the thread's bottom edge rather than scrolling with its content.
 *
 * It subscribes to the thread's end itself instead of taking it as a prop: the
 * panel it sits in is the whole conversation, and a scroll that changes
 * nothing but this button must not re-render that.
 */
export function ScrollToBottomButton({ follow }: { follow: FollowBottom }) {
  const atBottom = useSyncExternalStore(follow.subscribeAtBottom, follow.getAtBottom);
  if (atBottom) return null;
  return (
    <button
      type="button"
      className="scroll-to-bottom"
      title="Back to the end of the conversation"
      onClick={() => follow.scrollToBottom()}
    >
      <ArrowDown size={14} />
    </button>
  );
}
