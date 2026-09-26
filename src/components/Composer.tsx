import { useEffect, useRef, useState, type ReactNode } from "react";
import { SendHorizontal, Square, ShieldCheck, Bot, Brain, CornerDownRight, X } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { ContextMeter } from "./ContextMeter";
import { SlashMenu } from "./SlashMenu";
import { commandFor, suggestCommands, type SlashCommand } from "../lib/slashCommands";
import { matches, shortcutText } from "../lib/shortcuts";
import type { ChatUsage, ContextUsage, ConversationMode } from "../lib/chat";
import { choiceKey, type ModelChoice } from "../hooks/useLlmSettings";
import { effortOptions } from "../lib/providerForm";
import "./Composer.css";

// Two permission states, not the prototype's three, because two is what
// `ApprovalPolicy` has: ask before anything that changes the tree, or run the
// whole turn without asking. A third label would be a control that moves and
// changes nothing. "Always allow this tool" is the third real state and it is
// not a chip — it is answered on the card, about one tool, in the moment.
const MODES: { value: string; label: string; hint: string }[] = [
  { value: "ask", label: "Ask", hint: "Confirm edits and commands" },
  { value: "auto", label: "Auto", hint: "Run the whole turn without asking" },
];

// What the agent may be this turn. The labels are the user's words for it;
// the values are what `domain::conversation_mode` deserializes.
const CONVERSATIONS: { value: ConversationMode; label: string; hint: string }[] = [
  { value: "agent", label: "Agent", hint: "Reads, edits files and runs commands" },
  { value: "plan", label: "Plan", hint: "Reads only and writes a plan" },
  { value: "ask", label: "Ask", hint: "Answers questions, changes nothing" },
];

type Props = {
  /** Drawn on the box's top edge — the folder the message will be worked on. */
  tab?: ReactNode;
  /** Sends the box. While a turn is running the same box steers it instead. */
  onSend: (text: string) => void;
  /** Holds the box until the running turn ends, to be sent as the next message. */
  onQueue?: (text: string) => void;
  /** What is waiting for the turn to end; a row taken out goes back to the box. */
  queued?: { id: number; text: string }[];
  onUnqueue?: (id: number) => void;
  onStop: () => void;
  running: boolean;
  /** Both chips live above this component: the backend has to be told, and a
      component does not call commands. */
  conversation: ConversationMode;
  onConversation: (mode: ConversationMode) => void;
  /** `true` when the turn runs without asking — `ApprovalPolicy::skip_all`. */
  unattended: boolean;
  onUnattended: (unattended: boolean) => void;
  /** Text put into the box from outside — a branch hands back its message.
      Replaces what was typed; `seq` makes the same text twice land twice. */
  draft?: { text: string; seq: number } | null;
  /** Text added after what is typed — a selection from the terminal. */
  quote?: { text: string; seq: number } | null;
  /** Every configured `provider/model`, the one turns go to now, and its thinking level. */
  models: { choices: ModelChoice[]; current: ModelChoice | null; effort?: string | null };
  onModel: (choice: ModelChoice) => void;
  /** `null` is the model's default — nothing sent. */
  onEffort: (effort: string | null) => void;
  /** Asks the providers what they serve; called when the model menu opens. */
  onLoadModels: () => void;
  /** What the next request will cost, as the backend's own estimate — the one
      that decides when a conversation is folded. */
  context: ContextUsage | null;
  usage: ChatUsage | null;
  onCompact: () => void;
  /** What `/name` in the box runs instead of sending it; offered in a menu as it is typed. */
  commands?: SlashCommand[];
  /** Called as the `/` menu opens — for commands read from files, which change outside the app. */
  onCommandsOpen?: () => void;
  /** Each change puts the cursor in the box. */
  focus?: number;
};

export function Composer({
  tab,
  onSend,
  onQueue,
  queued = [],
  onUnqueue,
  onStop,
  running,
  conversation,
  onConversation,
  unattended,
  onUnattended,
  draft = null,
  quote = null,
  models,
  onModel,
  onEffort,
  onLoadModels,
  context,
  usage,
  onCompact,
  commands = [],
  onCommandsOpen,
  focus = 0,
}: Props) {
  const [text, setText] = useState("");
  const area = useRef<HTMLTextAreaElement>(null);
  const box = useRef<HTMLElement>(null);
  // The menu follows the text: shown while a name is being typed, until
  // Escape or a click elsewhere — and back with the next keystroke.
  const [dismissed, setDismissed] = useState(false);
  const [active, setActive] = useState(0);
  const offered = dismissed ? [] : suggestCommands(commands, text);
  const menuOpen = offered.length > 0;
  const at = Math.min(active, Math.max(offered.length - 1, 0));

  useEffect(() => {
    if (!menuOpen) return;
    onCommandsOpen?.();
    const onDown = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setDismissed(true);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [menuOpen]);

  const grow = () => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 120)}px`;
  };

  useEffect(() => {
    if (!draft) return;
    setText(draft.text);
    area.current?.focus();
    requestAnimationFrame(grow);
  }, [draft]);

  useEffect(() => {
    if (focus) area.current?.focus();
  }, [focus]);

  useEffect(() => {
    if (!quote) return;
    setText((typed) => (typed.trim() ? `${typed.trimEnd()}\n\n${quote.text}` : quote.text));
    area.current?.focus();
    requestAnimationFrame(grow);
  }, [quote]);

  const effort = models.effort ?? "";
  const efforts = effortOptions(effort);

  const clear = () => {
    setText("");
    requestAnimationFrame(grow);
  };

  // A command that cannot run now stays in the box, its menu row saying why.
  const run = (command: SlashCommand, args = "") => {
    if (command.unavailable) return;
    command.run(args);
    clear();
  };

  const send = () => {
    if (!text.trim()) return;
    const typed = commandFor(commands, text);
    if (typed) return run(typed.command, typed.args);
    onSend(text);
    clear();
  };

  // A `/` command is not queued: it runs now, as with Enter — the queue holds
  // text for the model, and a command's expansion belongs to the moment it runs.
  const queue = () => {
    if (!running || !onQueue || commandFor(commands, text)) return send();
    if (!text.trim()) return;
    onQueue(text);
    clear();
  };

  const complete = (command: SlashCommand) => {
    setText(`/${command.name} `);
    area.current?.focus();
  };

  return (
    <div className="composer-wrap">
      {/* Above the folder tab, which sits on the box's edge. */}
      {queued.length > 0 && (
        <ul className="composer-queue" aria-label="Queued messages">
          {queued.map((item) => (
            <li key={item.id}>
              <CornerDownRight size={13} />
              <span className="composer-queue-text">{item.text}</span>
              <button type="button" title="Back to the message box" aria-label="Back to the message box" onClick={() => onUnqueue?.(item.id)}>
                <X size={13} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {tab}
      <section className="composer" ref={box}>
        {menuOpen && <SlashMenu commands={offered} active={at} onPick={(c) => run(c)} />}
        <textarea
          className="chat-text"
          ref={area}
          rows={2}
          placeholder={
            running
              ? `Add something while it works — ${shortcutText("queue")} to send it after`
              : "Describe your task…"
          }
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            setDismissed(false);
            setActive(0);
            grow();
          }}
          onKeyDown={(e) => {
            if (menuOpen) {
              const step = matches(e, "commandNext") ? 1 : matches(e, "commandPrev") ? -1 : 0;
              if (step) {
                e.preventDefault();
                setActive((at + step + offered.length) % offered.length);
                return;
              }
              if (matches(e, "commandComplete")) {
                e.preventDefault();
                complete(offered[at]);
                return;
              }
              if (matches(e, "close")) {
                // The menu's Escape, not the window's.
                e.preventDefault();
                e.stopPropagation();
                setDismissed(true);
                return;
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                run(offered[at]);
                return;
              }
            }
            if (matches(e, "queue")) {
              e.preventDefault();
              queue();
            } else if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="composer-bar">
          <Dropdown
            title="Permission mode"
            heading="Permissions"
            label={
              <span className="mode-label">
                <ShieldCheck size={13} />
                {unattended ? "Auto" : "Ask"}
              </span>
            }
            value={unattended ? "auto" : "ask"}
            options={MODES}
            onPick={(v) => onUnattended(v === "auto")}
          />
          {/* Two chips because these are two questions. This one is what the
              agent is for; the one beside it is who has to agree before it
              acts — a plan-mode turn still asks, and an unattended agent still
              cannot write in Ask. */}
          <Dropdown
            title="What the agent may do"
            heading="Mode"
            label={
              <span className="mode-label">
                <Bot size={13} />
                {CONVERSATIONS.find((c) => c.value === conversation)?.label ?? "Agent"}
              </span>
            }
            value={conversation}
            options={CONVERSATIONS.map((c) => ({ value: c.value, label: c.label, hint: c.hint }))}
            onPick={(v) => onConversation(v as ConversationMode)}
          />
          <Dropdown
            title="Model"
            heading="Model"
            label={<span className="model-label">{models.current?.label ?? "no model"}</span>}
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
          {/* The active provider's setting, as in Settings → Models — so it follows
              the model it was set for, and a switch back finds it still set. */}
          {models.current && (
            <Dropdown
              title="Thinking level"
              heading="Thinking"
              label={
                <span className="mode-label">
                  <Brain size={13} />
                  {efforts.find((e) => e.value === effort)?.label}
                </span>
              }
              value={effort}
              options={efforts}
              onPick={(v) => onEffort(v || null)}
            />
          )}
          {/* Shown from the first render, before anything has been said: an
              empty conversation already costs the prompt and the schemas. */}
          {context && (
            <span className="composer-meter">
              <ContextMeter context={context} usage={usage} running={running} onCompact={onCompact} up />
            </span>
          )}
          {/* One button, two jobs: while a turn runs the only thing worth doing
              with it is stopping — and a send button that does nothing during a
              turn is worse than no button. */}
          {running ? (
            <button className="send stop" type="button" title="Stop" onClick={onStop}>
              <Square size={14} />
            </button>
          ) : (
            <button className="send" type="button" title="Send" onClick={send}>
              <SendHorizontal size={16} />
            </button>
          )}
        </div>
      </section>
    </div>
  );
}
