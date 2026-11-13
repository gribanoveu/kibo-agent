import { useCallback, useEffect, useMemo, useRef } from "react";
import { AppWindow, MessagesSquare, MousePointerClick, SendHorizontal, ShipWheel, Square, X, type LucideIcon } from "lucide-react";
import { AttachImagesButton } from "./AttachImagesButton";
import { ComposerSettings } from "./ComposerSettings";
import { Dropdown } from "./Dropdown";
import { KubePanel } from "./KubePanel";
import { ContextMeter } from "./ContextMeter";
import { ProviderBalance } from "./ProviderBalance";
import { Transcript } from "./ChatPanel";
import { KiboBrand } from "./KiboBrand";
import { matches } from "../lib/shortcuts";
import { greetingsAt, pickSuggestions, ROLE_SUGGESTIONS } from "../lib/chatSuggestions";
import type { ChatRoleId } from "../lib/plainChat";
import { boxTakesPictures } from "../lib/images";
import type { Pointed } from "../lib/prototype";
import { useAttachments } from "../hooks/useAttachments";
import { ImageThumbs } from "./ImageThumbs";
import { LayoutCorrectionHint } from "./LayoutCorrectionHint";
import { DictateButton } from "./DictateButton";
import { ErrorNote } from "./ErrorNote";
import { DictationLine } from "./VoiceWave";
import type { PlainChatState } from "../hooks/usePlainChat";
import type { ModelChoice } from "../hooks/useLlmSettings";
import { useFollowBottom } from "../hooks/useFollowBottom";
import { useArrival } from "../hooks/useArrival";
import { chatDraftKey, useChatDraft } from "../hooks/useChatDraft";
import { useDictation } from "../hooks/useDictation";
import { useLayoutCorrection } from "../hooks/useLayoutCorrection";
import type { KubeconfigsState } from "../hooks/useKubeconfigs";
import "./PlainChat.css";


/** Each role's sign, on its chip above the message box and in the role's menu. */
const ROLE_ICONS: Record<ChatRoleId, LucideIcon> = {
  assistant: MessagesSquare,
  kubernetes: ShipWheel,
  prototype: AppWindow,
};

/** What the empty chat says the role can do — longer than its menu hint, and plain about the limits. */
const ROLE_WELCOME: Record<ChatRoleId, string> = {
  assistant: "The model answers in text here: it does not read your files, run commands or change anything.",
  kubernetes:
    "The model reads the cluster picked below: pods, events, logs. With Allow changes on it can change it too, asking you first.",
  prototype:
    "The model builds a page — a site, a small app, a slide deck — as files in this chat's own folder. It reaches nothing else on your machine.",
};

type Props = {
  chat: PlainChatState;
  /** Each change puts the cursor in the box. */
  focus: number;
  /** The same models as the agent's box: the provider is the app's, not the mode's. */
  models: { choices: ModelChoice[]; current: ModelChoice | null; effort?: string | null };
  /** Whether that model is set to see pictures — the composer's own rule. */
  imagesEnabled?: boolean;
  onModel: (choice: ModelChoice) => void;
  onEffort: (effort: string | null) => void;
  onLoadModels: () => void;
  /** The Kubernetes role's cluster, picked in the panel beside the role. */
  kube: KubeconfigsState;
  /** Opens Settings → Kubernetes. */
  onSetUpKube: () => void;
  /** "Compact now" on the context meter. */
  onCompact: () => void;
  /** Asks to take the chat back to before a bubble — the dialog says what that does first. */
  onRewind?: (bubbleId: string) => void;
  /** A Prototype chat's Preview panel, beside the chat: absent for another role. */
  pages?: {
    /** Something was presented, so there is a page to show. */
    available: boolean;
    open: boolean;
    onToggle: () => void;
    onOpenVersion: (version: number) => void;
    /** A page's error to put in the message box; a new object each ask. */
    fix: { text: string } | null;
    /** The element pointed at in the preview, sent with the next message. */
    pointed: Pointed | null;
    onUnpoint: () => void;
    /** Opens the panel on its pictures: some are being attached to a message. */
    onShowAssets: () => void;
  };
  /** Whether Voice input is on: its button, its key. */
  dictate?: boolean;
  /** Whether the wrong-layout hint is on. Corrections are never automatic. */
  layoutCorrection?: boolean;
  /** Whether Fix also selects the corrected prose's dominant system layout. */
  switchLayoutAfterFix?: boolean;
  onLayoutCorrectionError?: (message: string) => void;
};


/**
 * Chat mode: the conversation and its message box, the two panels of `.main`.
 * No folder — the role says who the model is and what it may call; its calls
 * and their approval cards are drawn by the agent's own transcript.
 */
export function PlainChat({
  chat,
  focus,
  models,
  imagesEnabled = false,
  onModel,
  onEffort,
  onLoadModels,
  kube,
  onSetUpKube,
  onCompact,
  onRewind,
  pages,
  dictate = false,
  layoutCorrection = false,
  switchLayoutAfterFix = false,
  onLayoutCorrectionError,
}: Props) {
  // The box belongs to this chat, as the agent's does: what was typed here
  // waits here while another chat is open, and comes back with this one.
  const draftKey = chatDraftKey("chat", chat.chatId);
  const [draft, setDraft, take] = useChatDraft(draftKey);
  // One rule for both boxes, in `boxTakesPictures`: the hook's `enabled` is
  // what the button, the paste, the drop and the give-back all follow. Its
  // `key` is what keeps one chat's attached pictures off the next one's.
  const attachments = useAttachments(boxTakesPictures(imagesEnabled, chat.role), draftKey);
  const input = useRef<HTMLTextAreaElement>(null);
  // The box measures itself on every change of the text, so there is nothing to tell it.
  const dictation = useDictation({ on: dictate, draftKey, text: draft, setText: setDraft, area: input, onChange: () => {} });
  const correction = useLayoutCorrection({
    on: layoutCorrection && !dictation.active,
    text: draft,
    setText: setDraft,
    area: input,
    draftKey,
    switchLayout: switchLayoutAfterFix,
    onError: onLayoutCorrectionError,
  });
  /** A send waiting for a picture to be read: a second Enter is the same message, not a copy of it. */
  const waiting = useRef(false);
  const running = chat.turn.status === "running";
  const RoleIcon = ROLE_ICONS[chat.role];
  const empty = chat.turn.blocks.length === 0;
  // Two of the role's set, picked again each time an empty chat opens or the role changes.
  const suggestions = useMemo(() => pickSuggestions(ROLE_SUGGESTIONS[chat.role], 2), [chat.chatId, chat.role, empty]);
  // A greeting per empty chat, not per render: it must not change while the user types.
  const greeting = useMemo(() => pickSuggestions(greetingsAt(new Date().getHours()), 1)[0], [chat.chatId, empty]);

  useEffect(() => input.current?.focus(), [focus]);
  // A rewound message comes back to be changed and sent again, in place of what was there.
  useArrival(chat.draft, (rewound) => {
    setDraft(rewound.text);
    input.current?.focus();
  });
  // "Fix" under the preview: the error joins what is being written, not replaces it.
  useArrival(pages?.fix ?? null, (fix) => {
    setDraft((was) => (was.trim() ? `${was.trimEnd()}\n${fix.text}` : fix.text));
    input.current?.focus();
  });
  // Kept at the newest line while it is written, until the user scrolls up to
  // read; a message they send, or another chat, follows the end again.
  const { scrollRef, contentRef, scrollToBottom } = useFollowBottom();
  const sent = chat.turn.blocks.filter((b) => b.kind === "user").length;
  useEffect(() => {
    scrollToBottom();
  }, [chat.chatId, sent, scrollToBottom]);

  const send = async () => {
    if (chat.busy || waiting.current || dictation.active) return;
    if (!draft.trim() && attachments.images.length === 0 && attachments.preparing === 0) return;
    // What is sent is what was in the box at the keystroke; a picture still
    // being read is waited for, so it does not end up in the next message.
    const message = draft;
    waiting.current = true;
    try {
      if (attachments.preparing > 0) await attachments.settle();
      const attached = attachments.takeNamed();
      if (!message.trim() && attached.length === 0) return;
      if (attached.length > 0) pages?.onShowAssets();
      void chat.send(message, attached, pages?.pointed ?? null, imagesEnabled);
      pages?.onUnpoint();
      // The chat gets its id as the message goes: the draft to clear is this one,
      // not the one the box is about to show.
      take();
    } finally {
      waiting.current = false;
    }
  };
  // Pointed at in the preview: the message box is where the request about it is written.
  const pointed = pages?.pointed ?? null;
  useEffect(() => {
    if (pointed) input.current?.focus();
  }, [pointed]);
  const roleName = chat.roles.find((r) => r.id === chat.role)?.name ?? "Assistant";
  // The box fits what is in it, wherever it came from.
  const fit = useCallback(() => {
    const el = input.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
    // A line's height is a fraction: under the cap the rounding would draw a scrollbar.
    el.style.overflowY = el.scrollHeight > 160 ? "auto" : "hidden";
  }, []);

  useEffect(() => {
    fit();
  }, [draft, fit]);

  // The column's width moves with the layout (IDE layout): a wrapped line keeps
  // its second line, a freed one keeps the paid-for height, until the text
  // changes — the width itself is what re-fits the box.
  useEffect(() => {
    const el = input.current;
    if (!el) return;
    let width = el.clientWidth;
    const sized = new ResizeObserver(() => {
      if (el.clientWidth === width) return;
      width = el.clientWidth;
      fit();
    });
    sized.observe(el);
    return () => sized.disconnect();
  }, [fit]);

  return (
    <>
      <section className="plain-panel">

        <div ref={scrollRef} className="plain-thread chat-text">
          {empty ? (
            <div className="plain-welcome">
              <KiboBrand />
              <h2>{greeting}</h2>
              <p>{ROLE_WELCOME[chat.role]}</p>
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
            <div ref={contentRef}>
              <Transcript
                turn={chat.turn}
                onDecide={chat.decide}
                speaker={roleName}
                preview={chat.preview}
                branchable={chat.branchable}
                onRewind={onRewind}
                onOpenVersion={pages?.onOpenVersion}
              />
              {chat.error && <div className="plain-error">{chat.error}</div>}
            </div>
          )}
        </div>
      </section>

      <div className="plain-composer-wrap">
        <section className={`plain-composer${attachments.dragging ? " dropping" : ""}`}>
          {attachments.dragging && (
            <div className="plain-drop" aria-hidden="true">
              Drop to attach
            </div>
          )}
          <ImageThumbs images={attachments.images} preparing={attachments.preparing} onRemove={attachments.remove} />
          {pointed && (
            <div className="plain-pointed" title={pointed.text || pointed.selector}>
              <MousePointerClick size={12} />
              <code className="plain-pointed-selector">{pointed.selector}</code>
              <button type="button" className="plain-pointed-drop" title="Don't send the element" onClick={pages?.onUnpoint}>
                <X size={12} />
              </button>
            </div>
          )}
          {attachments.error && (
            <ErrorNote className="plain-attach-error" message={attachments.error} onClose={attachments.clearError} />
          )}
          {dictation.error && <ErrorNote className="plain-attach-error" message={dictation.error} onClose={dictation.clearError} />}
          <div className="plain-composer-bar">
            {/* The same rule as in the agent's box: the hook decides, and
                there is no button when it refuses. */}
            {attachments.enabled && <AttachImagesButton onPick={attachments.pick} />}
            <textarea
              ref={input}
              className="chat-text"
              rows={1}
              readOnly={dictation.active}
              placeholder="Message the model…"
              value={draft}
              onPaste={(e) => attachments.paste(Array.from(e.clipboardData.files), e)}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={correction.check}
              onKeyDown={(e) => {
                if (matches(e, "convertLayout") && correction.apply()) {
                  e.preventDefault();
                  return;
                }
                if (matches(e, "undo") && correction.undo()) {
                  e.preventDefault();
                  return;
                }
                if (matches(e, "send")) {
                  e.preventDefault();
                  void send();
                }
              }}
            />
            {chat.context && (
              <span className="plain-meter">
                <ContextMeter context={chat.context} usage={chat.turn.usage} running={chat.busy} onCompact={onCompact} up spent={chat.spent} />
              </span>
            )}
            {dictate && <DictateButton dictation={dictation} />}
            {running ? (
              <button type="button" className="plain-send stop" title="Stop" onClick={chat.stop}>
                <Square size={14} />
              </button>
            ) : (
              <button
                type="button"
                className="plain-send"
                title="Send"
                disabled={chat.busy || dictation.active || (!draft.trim() && attachments.images.length === 0 && attachments.preparing === 0)}
                onClick={send}
              >
                <SendHorizontal size={16} />
              </button>
            )}
          </div>
          {dictation.active && <DictationLine peaks={dictation.peaks} partial={dictation.partial} />}
        </section>
        {correction.suggestion && (
          <LayoutCorrectionHint corrected={correction.suggestion.corrected} onApply={correction.apply} />
        )}
        {/* Under the box, where the agent's says which folder: here, who the
            model is. The chat's own settings go on this strip as they come. */}
        <div className="plain-tab">
          {/* One role per chat: what was said was said to it. */}
          <Dropdown
            title={empty ? "Who the model is in this chat" : "The chat's role — start a new chat for another"}
            locked={!empty}
            heading="Role"
            label={
              <span className="plain-chip-label">
                <RoleIcon size={13} />
                <span>{roleName}</span>
              </span>
            }
            value={chat.role}
            options={chat.roles.map((r) => ({ value: r.id, label: r.name, hint: r.description }))}
            onPick={(value) => chat.setRole(value as ChatRoleId)}
          />
          {chat.role === "kubernetes" && (
            <KubePanel pin={chat.kube} onPin={chat.setPin} kube={kube} onSetUpKube={onSetUpKube} />
          )}
          {pages?.available && (
            <button
              type="button"
              className={`plain-preview-toggle${pages.open ? " on" : ""}`}
              title={pages.open ? "Hide the preview" : "Show the preview"}
              onClick={pages.onToggle}
            >
              <AppWindow size={13} />
              <span>Preview</span>
            </button>
          )}
          <ComposerSettings models={models} onModel={onModel} onEffort={onEffort} onLoadModels={onLoadModels} />
          <ProviderBalance providerId={models.current?.providerId ?? null} running={chat.busy} />
        </div>
      </div>
    </>
  );
}
