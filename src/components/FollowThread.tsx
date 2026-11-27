import { useEffect, useRef, type ReactNode } from "react";
import { useFollowBottom } from "../hooks/useFollowBottom";
import { ScrollToBottomButton } from "./ScrollToBottomButton";

/**
 * The thread of a chat: the scrolling box, what is in it, and the button back
 * to its end.
 *
 * It owns the following hook, and with it every state a scroll moves — the
 * library's own and the button's — so the panel around the whole conversation
 * does not repaint when the reader scrolls. `children` is what the panel put in
 * the thread, and is left alone: the same element comes back on every re-render
 * here, which React reconciles away.
 *
 * `open` names the chat on screen and `sent` changes when the reader sends
 * something or steers a note: another chat lands at its end at once, while
 * what they just sent glides there (`docs/65-stick-to-bottom.md`, SB-1c).
 */
export function FollowThread({
  className,
  empty,
  open,
  sent,
  children,
}: {
  /** The box's classes, as the panel names them. */
  className: string;
  /** What the thread shows while the conversation is still empty. */
  empty?: ReactNode;
  /** The chat on screen: a change of it opens a thread at its end at once. */
  open: string | null;
  /** What the reader sent, or steered, or the end of: follows the end again. */
  sent: string | undefined;
  children: ReactNode;
}) {
  const follow = useFollowBottom();
  const { scrollRef, contentRef, scrollToBottom } = follow;
  const followed = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    // A chat no thread has followed yet is being opened, even the first one.
    const opening = followed.current !== open;
    followed.current = open;
    scrollToBottom(opening);
  }, [open, sent, scrollToBottom]);
  return (
    <>
      <div ref={scrollRef} className={className}>
        {empty ?? (
          <div ref={contentRef} className="thread-content">
            {children}
          </div>
        )}
      </div>
      <ScrollToBottomButton follow={follow} />
    </>
  );
}
