import { useEffect, useMemo, useRef, useState } from "react";
import { Brain, ChevronRight, SendHorizontal, Square, UserRound } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Markdown } from "./Markdown";
import logo from "../assets/kibo-chat-logo.png";
import { matches } from "../lib/shortcuts";
import { pickSuggestions } from "../lib/chatSuggestions";
import type { ChatRoleId } from "../lib/chat";
import type { PlainChatState } from "../hooks/usePlainChat";
import { choiceKey, type ModelChoice } from "../hooks/useLlmSettings";
import { effortOptions } from "../lib/providerForm";
import "./PlainChat.css";

/** The model's thinking before an answer, folded until asked — as the agent's chat draws it. */
function Thinking({ text }: { text: string }) {
  return (
    <details className="plain-reasoning">
      <summary>
        <Brain size={13} />
        <span className="plain-reasoning-name">Thinking</span>
        <span className="plain-reasoning-preview">{text.split("\n", 1)[0]}</span>
        <ChevronRight className="plain-reasoning-chev" size={12} />
      </summary>
      <p className="plain-reasoning-text">{text}</p>
    </details>
  );
}

type Props = {
  chat: PlainChatState;
  /** Each change puts the cursor in the box. */
  focus: number;
  /** The same models as the agent's box: the provider is the app's, not the mode's. */
  models: { choices: ModelChoice[]; current: ModelChoice | null; effort?: string | null };
  onModel: (choice: ModelChoice) => void;
  onEffort: (effort: string | null) => void;
  onLoadModels: () => void;
};

/** Chat mode: the conversation and its message box, the two panels of `.main`. No folder, no tools — the role says what the model is. */
export function PlainChat({ chat, focus, models, onModel, onEffort, onLoadModels }: Props) {
  const [draft, setDraft] = useState("");
  const input = useRef<HTMLTextAreaElement>(null);
  const thread = useRef<HTMLDivElement>(null);
  const running = chat.streaming !== null;
  const empty = chat.messages.length === 0;
  // Two of the set, picked again each time an empty chat opens.
  const suggestions = useMemo(() => pickSuggestions(2), [chat.chatId, empty]);

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
  const effort = models.effort ?? "";
  const efforts = effortOptions(effort);

  return (
    <>
      <section className="plain-panel">
        <header className="plain-head">
          <h1>{chat.chats.find((c) => c.id === chat.chatId)?.title ?? "New chat"}</h1>
        </header>

        <div ref={thread} className="plain-thread chat-text">
          {chat.messages.length === 0 && !running ? (
            <div className="plain-welcome">
              <img className="plain-welcome-logo" src={logo} alt="" />
              <h2>How can I help?</h2>
              <p>The model answers in text here: it does not read your files, run commands or change anything.</p>
              <div className="plain-suggest">
                {suggestions.map((text) => (
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
                    {message.reasoning && <Thinking text={message.reasoning} />}
                    <Markdown text={message.text} streaming={false} />
                  </div>
                ),
              )}
              {chat.streaming && (
                <div className="plain-msg">
                  {chat.streaming.reasoning && <Thinking text={chat.streaming.reasoning} />}
                  {chat.streaming.text ? (
                    <Markdown text={chat.streaming.text} streaming />
                  ) : (
                    !chat.streaming.reasoning && (
                      <div className="plain-typing" aria-label="Writing">
                        <i />
                        <i />
                        <i />
                      </div>
                    )
                  )}
                </div>
              )}
              {chat.error && <div className="plain-error">{chat.error}</div>}
            </>
          )}
        </div>
      </section>

      <div className="plain-composer-wrap">
        {/* On the box's top edge, where the agent's says which folder: here, who the
            model is. The chat's own settings go on this strip as they come. */}
        <div className="plain-tab">
          <Dropdown
            title="Who the model is in this chat"
            heading="Role"
            label={
              <span className="plain-chip-label">
                <UserRound size={13} />
                {roleName}
              </span>
            }
            value={chat.role}
            options={chat.roles.map((r) => ({ value: r.id, label: r.name, hint: r.description }))}
            onPick={(value) => chat.setRole(value as ChatRoleId)}
          />
        </div>
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
            <Dropdown
              title="Model"
              heading="Model"
              label={<span className="plain-model-label">{models.current?.label ?? "no model"}</span>}
              value={models.current ? choiceKey(models.current) : ""}
              options={models.choices.map((choice) => ({
                value: choiceKey(choice),
                label: choice.label,
                hint: choice.model ? undefined : "The first model the provider lists",
              }))}
              emptyLabel="No provider yet — add one in Settings → Models"
              onOpen={onLoadModels}
              onPick={(key) => {
                const choice = models.choices.find((c) => choiceKey(c) === key);
                if (choice) onModel(choice);
              }}
            />
            {models.current && (
              <Dropdown
                title="Thinking level"
                heading="Thinking"
                label={
                  <span className="plain-chip-label">
                    <Brain size={13} />
                    {efforts.find((e) => e.value === effort)?.label}
                  </span>
                }
                value={effort}
                options={efforts}
                onPick={(v) => onEffort(v || null)}
              />
            )}
            {running ? (
              <button type="button" className="plain-send stop" title="Stop" onClick={chat.stop}>
                <Square size={14} />
              </button>
            ) : (
              <button type="button" className="plain-send" title="Send" disabled={!draft.trim()} onClick={send}>
                <SendHorizontal size={16} />
              </button>
            )}
          </div>
        </section>
      </div>
    </>
  );
}
