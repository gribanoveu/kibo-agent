import { useEffect, useRef, useState, type ReactNode } from "react";
import { SendHorizontal, Square, ShieldCheck, Bot, Brain, CornerDownRight, MessageSquareReply, X } from "lucide-react";
import { AttachImagesButton } from "./AttachImagesButton";
import { Dropdown } from "./Dropdown";
import { ContextMeter } from "./ContextMeter";
import { SlashMenu } from "./SlashMenu";
import { commandFor, pendingHint, suggestCommands, typingCommandName, type SlashCommand } from "../lib/slashCommands";
import { matches, shortcutText } from "../lib/shortcuts";
import { CONVERSATION_MODES, type ChatUsage, type ContextUsage, type ConversationMode } from "../lib/chat";
import { boxTakesPictures, type ImagePart } from "../lib/images";
import { choiceKey, type ModelChoice } from "../hooks/useLlmSettings";
import { effortOptions } from "../lib/providerForm";
import { useAttachments } from "../hooks/useAttachments";
import { useDictation } from "../hooks/useDictation";
import { useArrival } from "../hooks/useArrival";
import { useChatDraft } from "../hooks/useChatDraft";
import { DictateButton } from "./DictateButton";
import { VoiceWave } from "./VoiceWave";
import { ImageThumbs } from "./ImageThumbs";
import { ErrorNote } from "./ErrorNote";
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

type Props = {
  /** Drawn on the box's top edge — the folder the message will be worked on. */
  tab?: ReactNode;
  /** Sends the box, with the pictures attached to it. While a turn is running
      the same box steers it instead, and the pictures go with the note. */
  onSend: (text: string, images: ImagePart[]) => void;
  /** Whether the model turns go to is set to see pictures; off, a pasted one
      is refused with a word on where to change that. */
  imagesEnabled?: boolean;
  /** Holds the box until the running turn ends, to be sent as the next message.
      The pictures attached to it are held with it. */
  onQueue?: (text: string, images: ImagePart[]) => void;
  /** What is waiting for the turn to end; a row taken out goes back to the box. */
  queued?: { id: number; text: string; images?: ImagePart[] }[];
  onUnqueue?: (id: number) => void;
  /** Notes sent into the running turn that no round has read yet; one taken
      back goes to the box, with the pictures it carried. */
  steered?: { id: string; text: string; images?: ImagePart[] }[];
  onWithdraw?: (id: string) => void;
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
      Replaces what was typed; `seq` makes the same text twice land twice, and
      `images` come back with a note taken out of a running turn. */
  draft?: { text: string; seq: number; images?: ImagePart[] } | null;
  /** Text added after what is typed — a selection from the terminal. */
  quote?: { text: string; seq: number; images?: ImagePart[] } | null;
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
  /** Said at the end of the `/` menu while more commands are on their way; keeps it open meanwhile. */
  pendingCommands?: string;
  /** Each change puts the cursor in the box. */
  focus?: number;
  /** Where this chat's unsent message is kept: the box belongs to a chat, not
      to the window, so it comes back to the one it was typed in. */
  draftKey: string;
  /** Whether Voice input is on: its button, its key. */
  dictate?: boolean;
};

export function Composer({
  tab,
  onSend,
  imagesEnabled = false,
  onQueue,
  queued = [],
  onUnqueue,
  steered = [],
  onWithdraw,
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
  pendingCommands,
  focus = 0,
  draftKey,
  dictate = false,
}: Props) {
  const [text, setText, take] = useChatDraft(draftKey);
  /** The queued row shown whole; the rest are a line each. */
  const [openRow, setOpenRow] = useState<string | null>(null);
  /** A send waiting for a picture to be read: a second Enter is the same message, not a copy of it. */
  const waiting = useRef(false);
  const attachments = useAttachments(boxTakesPictures(imagesEnabled), draftKey);
  const area = useRef<HTMLTextAreaElement>(null);
  const box = useRef<HTMLElement>(null);
  const dictation = useDictation({ on: dictate, draftKey, text, setText, area, onChange: () => requestAnimationFrame(grow) });
  // The menu follows the text: shown while a name is being typed, until
  // Escape or a click elsewhere — and back with the next keystroke. What is
  // *typed*, not what the box holds: a draft restored as a chat opens is not
  // being typed, so its menu waits. `typedIn` is the chat whose text in the box
  // was typed here.
  const [typedIn, setTypedIn] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const offered = typedIn === draftKey && !dictation.active ? suggestCommands(commands, text) : [];
  // Open while more commands are coming, even if none typed so far matches
  // yet: the prompt being typed may be one of them.
  const menuOpen = offered.length > 0 || (!!pendingCommands && typedIn === draftKey && !dictation.active && typingCommandName(text));
  const at = Math.min(active, Math.max(offered.length - 1, 0));
  const hint = pendingHint(commands, text);

  useEffect(() => {
    if (!menuOpen) return;
    onCommandsOpen?.();
    const onDown = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setTypedIn(null);
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

  // Another chat's draft was put in the box during the render, so the box has
  // to be made to fit it — the textarea grew for the text it had before.
  useEffect(() => {
    setActive(0);
    requestAnimationFrame(grow);
  }, [draftKey]);

  useArrival(draft, (draft) => {
    setText(draft.text);
    // A draft replaces what was typed, pictures included — a picture left in
    // the box would otherwise ride along with the returned message.
    attachments.restore(draft.images ?? [], true);
    area.current?.focus();
    requestAnimationFrame(grow);
  });

  useEffect(() => {
    if (focus) area.current?.focus();
  }, [focus]);

  useArrival(quote, (quote) => {
    // A note taken back with only its picture has nothing to add to the text.
    const said = quote.text;
    if (said) setText((typed) => (typed.trim() ? `${typed.trimEnd()}\n\n${said}` : said));
    attachments.restore(quote.images ?? []);
    area.current?.focus();
    requestAnimationFrame(grow);
  });

  const effort = models.effort ?? "";
  const efforts = effortOptions(effort);

  const clear = () => {
    take();
    requestAnimationFrame(grow);
  };

  // A command that cannot run now stays in the box, its menu row saying why.
  const run = (command: SlashCommand, args = "") => {
    if (command.unavailable) return;
    command.run(args);
    clear();
  };

  const send = async () => {
    // What is said goes in the box first; Stop, then Send, as with anything typed.
    if (dictation.active) return;
    const typed = commandFor(commands, text);
    if (typed) return run(typed.command, typed.args);
    if (waiting.current) return;
    if (!text.trim() && attachments.images.length === 0 && attachments.preparing === 0) return;
    // What is said is what was in the box at the keystroke. A picture still
    // being read is part of it: the box waits for the picture rather than
    // sending the words alone and leaving it to ride with the next message.
    const message = text;
    waiting.current = true;
    try {
      if (attachments.preparing > 0) await attachments.settle();
      const pictures = attachments.take();
      if (!message.trim() && pictures.length === 0) return;
      onSend(message, pictures);
      clear();
    } finally {
      waiting.current = false;
    }
  };

  // A `/` command is not queued: it runs now, as with Enter — the queue holds
  // text for the model, and a command's expansion belongs to the moment it runs.
  const queue = async () => {
    if (dictation.active) return;
    if (!running || !onQueue || commandFor(commands, text)) return send();
    if (waiting.current) return;
    if (!text.trim() && attachments.images.length === 0 && attachments.preparing === 0) return;
    // A held message takes its pictures too — a picture still being read is
    // waited for, never left in the box to ride with the next message.
    const message = text;
    waiting.current = true;
    try {
      if (attachments.preparing > 0) await attachments.settle();
      const pictures = attachments.take();
      if (!message.trim() && pictures.length === 0) return;
      onQueue(message, pictures);
      clear();
    } finally {
      waiting.current = false;
    }
  };

  const complete = (command: SlashCommand) => {
    setText(`/${command.name} `);
    area.current?.focus();
  };

  // A command that takes arguments is put in the box to type them; one that
  // takes none runs. Run bare, `/compare` would only send its prompt with its
  // placeholders in it — and Enter again runs it once they are typed.
  const pick = (command: SlashCommand) => (command.argumentHint ? complete(command) : run(command));

  // A row of the queue: when it is read, said in words rather than in a
  // tooltip, and its text, cut to a line until clicked open.
  const queueRow = (key: string, icon: ReactNode, when: string, text: string, back: string, onBack: () => void) => {
    const open = openRow === key;
    return (
      <li key={key} className={open ? "open" : undefined}>
        {icon}
        <span className="composer-queue-when">{when}</span>
        <button type="button" className="composer-queue-text" aria-expanded={open} onClick={() => setOpenRow(open ? null : key)}>
          {text}
        </button>
        <button type="button" className="composer-queue-back" title={back} aria-label={back} onClick={onBack}>
          <X size={13} />
        </button>
      </li>
    );
  };

  /** What a row of the queue says: what was typed, and the pictures that go
      with it — a row is a line, so they are counted rather than drawn. A note
      can be a picture with nothing typed around it. */
  const rowText = (text: string, images?: ImagePart[]) => {
    const count = images?.length ?? 0;
    if (count === 0) return text;
    const said = count === 1 ? "a picture" : `${count} pictures`;
    return text ? `${text} · ${said}` : `${said[0].toUpperCase()}${said.slice(1)}`;
  };

  return (
    <div className="composer-wrap">
      {/* Above the folder tab, which sits on the box's edge. */}
      {(steered.length > 0 || queued.length > 0) && (
        <ul className="composer-queue" aria-label="Queued messages">
          {/* The notes first: they reach the agent at its next step, the
              queue only after the turn. */}
          {steered.map((note) =>
            queueRow(
              `note-${note.id}`,
              <MessageSquareReply size={13} />,
              "Next step",
              rowText(note.text, note.images),
              "Take back to the message box",
              () => onWithdraw?.(note.id),
            ),
          )}
          {queued.map((item) =>
            queueRow(
              `queued-${item.id}`,
              <CornerDownRight size={13} />,
              "After this turn",
              rowText(item.text, item.images),
              "Back to the message box",
              () => onUnqueue?.(item.id),
            ),
          )}
        </ul>
      )}
      {tab}
      <section className={`composer${attachments.dragging ? " dropping" : ""}`} ref={box}>
        {attachments.dragging && (
          <div className="composer-drop" aria-hidden="true">
            Drop to attach
          </div>
        )}
        {menuOpen && <SlashMenu commands={offered} active={at} onPick={pick} pending={pendingCommands} />}
        <ImageThumbs images={attachments.images} preparing={attachments.preparing} onRemove={attachments.remove} />
        {attachments.error && (
          <ErrorNote className="composer-attach-error" message={attachments.error} onClose={attachments.clearError} />
        )}
        {dictation.error && <ErrorNote className="composer-attach-error" message={dictation.error} onClose={dictation.clearError} />}
        <div className="composer-input">
          {/* The arguments still to type, in grey after the text: the text
              itself is drawn here too, invisibly, so the hint starts where it
              ends. The textarea above is transparent. `chat-text` for the
              chat's own font scale, which the textarea is drawn at. */}
          {hint && (
            <div className="composer-ghost chat-text" aria-hidden="true">
              <span>{text}</span>
              <span className="composer-ghost-hint">{hint}</span>
            </div>
          )}
          <textarea
            className="chat-text"
            ref={area}
            rows={2}
            readOnly={dictation.active}
            placeholder={
              running
                ? `Add something while it works — ${shortcutText("queue")} to send it after`
                : "Describe your task…"
            }
            value={text}
            onPaste={(e) => attachments.paste(Array.from(e.clipboardData.files), e)}
            onChange={(e) => {
              setText(e.target.value);
              setTypedIn(draftKey);
              setActive(0);
              grow();
            }}
            onKeyDown={(e) => {
              if (menuOpen) {
                // Only the line saying more is coming: nothing to move to or
                // pick, and Enter does what it does with the menu closed.
                const step = offered.length === 0 ? 0 : matches(e, "commandNext") ? 1 : matches(e, "commandPrev") ? -1 : 0;
                if (step) {
                  e.preventDefault();
                  setActive((at + step + offered.length) % offered.length);
                  return;
                }
                if (offered.length > 0 && matches(e, "commandComplete")) {
                  e.preventDefault();
                  complete(offered[at]);
                  return;
                }
                if (matches(e, "close")) {
                  // The menu's Escape, not the window's.
                  e.preventDefault();
                  e.stopPropagation();
                  setTypedIn(null);
                  return;
                }
                if (offered.length > 0 && matches(e, "send")) {
                  e.preventDefault();
                  pick(offered[at]);
                  return;
                }
              }
              if (matches(e, "queue")) {
                e.preventDefault();
                void queue();
              } else if (matches(e, "send")) {
                e.preventDefault();
                void send();
              }
            }}
          />
        </div>
        {dictation.active && (
          <div className="composer-dictation">
            <VoiceWave peaks={dictation.peaks} />
            {/* The phrase being said: it changes until it is whole, and then goes in the box. */}
            {dictation.partial && <p className="composer-partial chat-text">{dictation.partial}</p>}
          </div>
        )}
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
                {CONVERSATION_MODES.find((c) => c.value === conversation)?.label ?? "Agent"}
              </span>
            }
            value={conversation}
            options={CONVERSATION_MODES.map((c) => ({ value: c.value, label: c.label, hint: c.hint }))}
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
          {/* The button is the hook's own answer, not a second copy of the rule:
              whatever the box refuses to paste, drop or bring back, it does not
              offer to pick either. */}
          {attachments.enabled && <AttachImagesButton onPick={attachments.pick} />}
          {dictate && <DictateButton dictation={dictation} />}
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
