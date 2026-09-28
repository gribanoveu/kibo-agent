import { useEffect, useRef, useState } from "react";
import { SendHorizontal, Square } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Markdown } from "./Markdown";
import logo from "../assets/kibo-chat-logo.png";
import { matches } from "../lib/shortcuts";
import type { ChatRoleId } from "../lib/chat";
import type { PlainChatState } from "../hooks/usePlainChat";
import "./PlainChat.css";

const SUGGESTIONS = [
  "Explain the difference between a process and a thread",
  "Write a regex that matches an ISO 8601 date",
  "Make this commit message clearer for a reviewer",
];

/** Chat mode: the conversation and its message box, the two panels of `.main`. No folder, no tools — the role says what the model is. */
export function PlainChat({ chat, focus }: { chat: PlainChatState; focus: number }) {
  const [draft, setDraft] = useState("");
  const input = useRef<HTMLTextAreaElement>(null);
  const thread = useRef<HTMLDivElement>(null);
  const running = chat.streaming !== null;

  useEffect(() => input.current?.focus(), [focus]);
  // Kept at the newest line while it is written.
  useEffect(() => {
    thread.current?.scrollTo({ top: thread.current.scrollHeight });
  }, [chat.messages, chat.streaming]);

  const send = () => {
    if (running || !draft.trim()) return;
    void chat.send(draft);
    setDraft("");
  };
  const roleName = chat.roles.find((r) => r.id === chat.role)?.name ?? "Assistant";

  return (
    <>
      <section className="plain-panel">
        <header className="plain-head">
          <div>
            <h1>{chat.chats.find((c) => c.id === chat.chatId)?.title ?? "New chat"}</h1>
            <p className="plain-sub">Talks with the model — no files, commands or tools</p>
          </div>
          <div className="plain-head-right">
            {chat.roles.length > 1 ? (
              <Dropdown
                label={roleName}
                title="Who the model is in this chat"
                options={chat.roles.map((r) => ({ value: r.id, label: r.name }))}
                value={chat.role}
                onPick={(value) => chat.setRole(value as ChatRoleId)}
                below
                right
              />
            ) : (
              <span className="plain-role">{roleName}</span>
            )}
          </div>
        </header>

        <div ref={thread} className="plain-thread chat-text">
          {chat.messages.length === 0 && !running ? (
            <div className="plain-welcome">
              <img className="plain-welcome-logo" src={logo} alt="" />
              <h2>How can I help?</h2>
              <p>The model answers in text here: it does not read your files, run commands or change anything.</p>
              <div className="plain-suggest">
                {SUGGESTIONS.map((text) => (
                  <button
                    key={text}
                    type="button"
                    onClick={() => {
                      setDraft(text);
                      input.current?.focus();
                    }}
                  >
                    {text}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <>
              {chat.messages.map((message, i) =>
                message.role === "user" ? (
                  <div key={i} className="plain-msg me">
                    <div className="plain-text">{message.text}</div>
                  </div>
                ) : (
                  <div key={i} className="plain-msg">
                    <Markdown text={message.text} streaming={false} />
                  </div>
                ),
              )}
              {running && (
                <div className="plain-msg">
                  {chat.streaming ? (
                    <Markdown text={chat.streaming} streaming />
                  ) : (
                    <div className="plain-typing" aria-label="Writing">
                      <i />
                      <i />
                      <i />
                    </div>
                  )}
                </div>
              )}
              {chat.error && <div className="plain-error">{chat.error}</div>}
            </>
          )}
        </div>
      </section>

      <section className="plain-composer">
        <textarea
          ref={input}
          className="chat-text"
          rows={2}
          placeholder="Message the model…"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (matches(e, "send") && !e.nativeEvent.isComposing) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="plain-composer-bar">
          {running ? (
            <button type="button" className="iconbtn" title="Stop" onClick={chat.stop}>
              <Square size={14} />
            </button>
          ) : (
            <button type="button" className="iconbtn plain-send" title="Send" disabled={!draft.trim()} onClick={send}>
              <SendHorizontal size={16} />
            </button>
          )}
        </div>
      </section>
    </>
  );
}
