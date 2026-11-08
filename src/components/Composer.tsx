import { useEffect, useRef, useState, type ReactNode } from "react";
import { SendHorizontal, Square, ShieldCheck, CornerDownRight, GitFork, X } from "lucide-react";
import { AttachImagesButton } from "./AttachImagesButton";
import { ComposerSettings } from "./ComposerSettings";
import { ContextMeter } from "./ContextMeter";
import { IndexBadge } from "./IndexBadge";
import { SlashMenu } from "./SlashMenu";
import { commandFor, pendingHint, suggestCommands, typingCommandName, type SlashCommand } from "../lib/slashCommands";
import { matches, shortcutText } from "../lib/shortcuts";
import type { ChatUsage, ContextUsage, ConversationMode } from "../lib/chat";
import { boxTakesPictures, type ImagePart } from "../lib/images";
import type { IndexState } from "../lib/indexStatus";
import type { ModelChoice } from "../hooks/useLlmSettings";
import { useAttachments } from "../hooks/useAttachments";
import { useDictation } from "../hooks/useDictation";
import { useArrival } from "../hooks/useArrival";
import { useChatDraft } from "../hooks/useChatDraft";
import { DictateButton } from "./DictateButton";
import { DictationLine } from "./VoiceWave";
import { ImageThumbs } from "./ImageThumbs";
import { ErrorNote } from "./ErrorNote";
import { LayoutCorrectionHint } from "./LayoutCorrectionHint";
import { useLayoutCorrection } from "../hooks/useLayoutCorrection";
import "./Composer.css";

type Props = {
  /** Drawn under the box — the folder the message will be worked on. */
  tab?: ReactNode;
  /** At the right end of the line under the box — the provider's balance. */
  end?: ReactNode;
  /** The open folder's index, said in the settings panel; `null` until anything is known about it. */
  index?: IndexState | null;
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
  onStop: () => void;
  running: boolean;
  /** Both settings live above this component: the backend has to be told, and
      a component does not call commands. */
  conversation: ConversationMode;
  onConversation: (mode: ConversationMode) => void;
  /** `true` when the turn runs without asking — `ApprovalPolicy::skip_all`. */
  unattended: boolean;
  onUnattended: (unattended: boolean) => void;
  /** Offered only before the chat starts: a worktree is a new folder, and a new
      chat in it. `title` says where it would start. */
  worktree?: { on: boolean; title: string; onChange: (on: boolean) => void };
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
  /** Whether the wrong-layout hint is on. Corrections are never automatic. */
  layoutCorrection?: boolean;
  /** Whether Fix also selects the corrected prose's dominant system layout. */
  switchLayoutAfterFix?: boolean;
  onLayoutCorrectionError?: (message: string) => void;
};

export function Composer({
  tab,
  end,
  index = null,
  onSend,
  imagesEnabled = false,
  onQueue,
  queued = [],
  onUnqueue,
  onStop,
  running,
  conversation,
  onConversation,
  unattended,
  onUnattended,
  worktree,
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
  layoutCorrection = false,
  switchLayoutAfterFix = false,
  onLayoutCorrectionError,
}: Props) {
  const [text, setText, take] = useChatDraft(draftKey);
  /** The queued row shown whole; the rest are a line each. */
  const [openRow, setOpenRow] = useState<number | null>(null);
  /** A send waiting for a picture to be read: a second Enter is the same message, not a copy of it. */
  const waiting = useRef(false);
  const attachments = useAttachments(boxTakesPictures(imagesEnabled), draftKey);
  const area = useRef<HTMLTextAreaElement>(null);
  const box = useRef<HTMLElement>(null);
  const dictation = useDictation({ on: dictate, draftKey, text, setText, area, onChange: () => requestAnimationFrame(grow) });
  const correction = useLayoutCorrection({
    on: layoutCorrection && !dictation.active,
    text,
    setText,
    area,
    draftKey,
    switchLayout: switchLayoutAfterFix,
    onError: onLayoutCorrectionError,
  });
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
    // A line's height is a fraction: under the cap the rounding would draw a scrollbar.
    el.style.overflowY = el.scrollHeight > 120 ? "auto" : "hidden";
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

  // A card of the queue: what it is and when it is read, said in words rather
  // than in a tooltip, over its text, cut to a line until clicked open.
  const queueRow = (item: { id: number; text: string; images?: ImagePart[] }) => {
    const open = openRow === item.id;
    return (
      <li key={item.id}>
        <div className="composer-queue-head">
          <CornerDownRight size={13} />
          <span className="composer-queue-kind">Queued</span>
          <span className="composer-queue-when">After this turn</span>
          <button
            type="button"
            className="composer-queue-back"
            title="Back to the message box"
            aria-label="Back to the message box"
            onClick={() => onUnqueue?.(item.id)}
          >
            <X size={13} />
          </button>
        </div>
        <button
          type="button"
          className={`composer-queue-text${open ? " open" : ""}`}
          aria-expanded={open}
          onClick={() => setOpenRow(open ? null : item.id)}
        >
          {rowText(item.text, item.images)}
        </button>
      </li>
    );
  };

  /** What a row of the queue says: what was typed, and the pictures that go
      with it — a row is a line, so they are counted rather than drawn. A message
      can be a picture with nothing typed around it. */
  const rowText = (text: string, images?: ImagePart[]) => {
    const count = images?.length ?? 0;
    if (count === 0) return text;
    const said = count === 1 ? "a picture" : `${count} pictures`;
    return text ? `${text} · ${said}` : `${said[0].toUpperCase()}${said.slice(1)}`;
  };

  return (
    <div className="composer-wrap">
      {queued.length > 0 && (
        <ul className="composer-queue" aria-label="Queued messages">
          {queued.map((item) => queueRow(item))}
        </ul>
      )}
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
        <div className="composer-bar">
          {/* The button is the hook's own answer, not a second copy of the rule:
              whatever the box refuses to paste, drop or bring back, it does not
              offer to pick either. */}
          {attachments.enabled && <AttachImagesButton onPick={attachments.pick} />}
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
              rows={1}
              readOnly={dictation.active}
              placeholder={
                running
                  ? `Add something — ${shortcutText("queue")} to send it after`
                  : "Describe your task…"
              }
              value={text}
              onPaste={(e) => attachments.paste(Array.from(e.clipboardData.files), e)}
              onBlur={correction.check}
              onChange={(e) => {
                setText(e.target.value);
                setTypedIn(draftKey);
                setActive(0);
                grow();
              }}
              onKeyDown={(e) => {
                if (matches(e, "convertLayout") && correction.apply()) {
                  e.preventDefault();
                  return;
                }
                if (matches(e, "undo") && correction.undo()) {
                  e.preventDefault();
                  return;
                }
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
          {/* Shown from the first render, before anything has been said: an
              empty conversation already costs the prompt and the schemas. */}
          {context && (
            <span className="composer-meter">
              <ContextMeter context={context} usage={usage} running={running} onCompact={onCompact} up />
            </span>
          )}
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
        {dictation.active && <DictationLine peaks={dictation.peaks} partial={dictation.partial} />}
      </section>
      {correction.suggestion && <LayoutCorrectionHint corrected={correction.suggestion.corrected} onApply={correction.apply} />}
      <div className="composer-strip">
        {tab}
        <ComposerSettings
          models={models}
          onModel={onModel}
          onEffort={onEffort}
          onLoadModels={onLoadModels}
          mode={{ value: conversation, onPick: onConversation }}
          // Two states, because two is what `ApprovalPolicy` has: ask before
          // anything that changes the tree, or run the whole turn without
          // asking. "Always allow this tool" is the third real one, answered on
          // the card, about one tool, in the moment. Set in the panel; read on
          // the line, where Auto is not to be missed.
          note={<span>{unattended ? "auto" : "ask"}</span>}
          status={index && <IndexBadge state={index} />}
          toggles={[
            ...(worktree ? [{ label: "Worktree", icon: <GitFork size={14} />, ...worktree }] : []),
            {
              label: "Approve automatically",
              icon: <ShieldCheck size={14} />,
              title: "Run the whole turn without asking; off, edits and commands are confirmed",
              on: unattended,
              onChange: onUnattended,
            },
          ]}
        />
        {end}
      </div>
    </div>
  );
}
