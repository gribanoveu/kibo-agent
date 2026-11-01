import { type Dispatch, type SetStateAction, useCallback, useEffect, useState } from "react";

/** Which message box it is: the agent's one, or Chat mode's own. */
export type DraftMode = "agent" | "chat";

/**
 * Where one chat's unsent message is kept. The mode is part of it because the
 * two modes' chats are saved in stores of their own, with ids that only happen
 * to differ. The chat not saved yet has one slot per mode — that is what "New
 * chat" finds again what was typed in the one left.
 */
export const chatDraftKey = (mode: DraftMode, chatId: string | null) => `chat-draft.${mode}.${chatId || "new"}`;

/** What a chat's box had in it — nothing for a key nothing was written under. */
export function readChatDraft(key: string): string {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

/** An empty draft is not written: a chat whose message was sent keeps no row. */
function writeChatDraft(key: string, text: string) {
  try {
    if (text) localStorage.setItem(key, text);
    else localStorage.removeItem(key);
  } catch {
    // Storage refused — a full disk, a cleared profile. The draft is still in
    // the box for as long as the window is open, which is all it promised.
  }
}

/** The chat is gone: what was typed in it goes with what was said. */
export function forgetChatDraft(key: string) {
  try {
    localStorage.removeItem(key);
  } catch {
    // Storage refused, so it holds nothing of this chat either.
  }
}

/**
 * The message box's text for `key` — kept in this window's storage, so it is
 * the same after opening another chat, after switching between Agent and Chat,
 * and after the next launch.
 *
 * Every change is written under the key it was made for, so a key that changes
 * only has to read the one now open. That read happens in the render: done in
 * an effect, the box would draw the other chat's text for a frame first.
 *
 * The setter is React's own, so a change made from what is in the box already —
 * a quote added after what was typed — stays a function of the box, not of the
 * text this render happened to have.
 */
export function useChatDraft(key: string): [string, Dispatch<SetStateAction<string>>, () => void] {
  const [text, setText] = useState(() => readChatDraft(key));
  // Which key the text in the box belongs to. A state, not a ref, because the
  // box must be drawn again in the same render the text was read in.
  const [owner, setOwner] = useState(key);
  if (owner !== key) {
    setOwner(key);
    setText(readChatDraft(key));
  }

  // Written after the render that changed it rather than at the keystroke: the
  // box is filled from outside too — a rewound message, a quote, a page's
  // error. A key that changed was read in the render above, so what is written
  // here belongs to the chat now open.
  useEffect(() => {
    writeChatDraft(key, text);
  }, [key, text]);

  // The box emptied now — sent, or a command that started another chat. Written
  // under the chat the box shows at this moment: sending the first message of a
  // chat gives it an id in the same commit, and the draft that has to go is the
  // one the text was typed under, not the one now open.
  const take = useCallback(() => {
    setText("");
    writeChatDraft(key, "");
  }, [key]);

  return [text, setText, take];
}
