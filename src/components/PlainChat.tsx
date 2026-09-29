import { useEffect, useMemo, useRef, useState } from "react";
import { Brain, FileCog, Layers, MessagesSquare, SendHorizontal, Server, ShipWheel, Square, type LucideIcon } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { ContextMeter } from "./ContextMeter";
import { Transcript } from "./ChatPanel";
import logo from "../assets/kibo-chat-logo.png";
import { matches } from "../lib/shortcuts";
import { pickSuggestions } from "../lib/chatSuggestions";
import type { ChatRoleId } from "../lib/chat";
import type { PlainChatState } from "../hooks/usePlainChat";
import { choiceKey, type ModelChoice } from "../hooks/useLlmSettings";
import { effortOptions } from "../lib/providerForm";
import { useFollowBottom } from "../hooks/useFollowBottom";
import type { KubeconfigsState } from "../hooks/useKubeconfigs";
import { namespaceOptions, useKubeTarget } from "../hooks/useKubeTarget";
import { rememberNamespace } from "../lib/kube";
import "./PlainChat.css";

/** Each role's sign: in Kibo's cloud on the empty chat and on the role's tab. */
const ROLE_ICONS: Record<ChatRoleId, LucideIcon> = {
  assistant: MessagesSquare,
  kubernetes: ShipWheel,
};

// A thought cloud: overlapping circles, then the bubbles trailing to the head.
const CLOUD = [
  [24, 24, 13],
  [38, 17, 15],
  [53, 25, 12],
  [31, 35, 10],
  [46, 36, 10],
  [12, 47, 4.5],
  [5, 54, 2.5],
] as const;

/** Drawn twice — outlines, then fills over them — so only the outer edge shows. */
function Cloud() {
  const circles = CLOUD.map(([cx, cy, r]) => <circle key={`${cx}-${cy}`} cx={cx} cy={cy} r={r} />);
  return (
    <svg className="plain-cloud-shape" viewBox="0 0 72 58">
      <g className="plain-cloud-edge">{circles}</g>
      <g className="plain-cloud-fill">{circles}</g>
    </svg>
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
  /** The Kubernetes role's cluster, picked on the tab beside the role. */
  kube: KubeconfigsState;
  /** Opens Settings → Kubernetes. */
  onSetUpKube: () => void;
  /** "Compact now" on the context meter. */
  onCompact: () => void;
};

// Not a kubeconfig's name: the menu's last row, which opens Settings instead.
const MANAGE_KUBECONFIGS = "\u0000manage";

/**
 * Chat mode: the conversation and its message box, the two panels of `.main`.
 * No folder — the role says who the model is and what it may call; its calls
 * and their approval cards are drawn by the agent's own transcript.
 */
export function PlainChat({ chat, focus, models, onModel, onEffort, onLoadModels, kube, onSetUpKube, onCompact }: Props) {
  const [draft, setDraft] = useState("");
  const input = useRef<HTMLTextAreaElement>(null);
  const running = chat.turn.status === "running";
  const RoleIcon = ROLE_ICONS[chat.role];
  // The chat's own cluster: its kubeconfig from Settings, and what its pin resolves to.
  const config = kube.configs.find((c) => c.name === chat.kube?.kubeconfig) ?? null;
  const target = useKubeTarget(config ? chat.kube : null);
  const empty = chat.turn.blocks.length === 0;
  // Two of the set, picked again each time an empty chat opens.
  const suggestions = useMemo(() => pickSuggestions(2), [chat.chatId, empty]);

  useEffect(() => input.current?.focus(), [focus]);
  // Kept at the newest line while it is written, until the user scrolls up to
  // read; a message they send, or another chat, follows the end again.
  const { scrollRef, contentRef, scrollToBottom } = useFollowBottom();
  const sent = chat.turn.blocks.filter((b) => b.kind === "user").length;
  useEffect(() => {
    scrollToBottom();
  }, [chat.chatId, sent, scrollToBottom]);

  const send = () => {
    if (chat.busy || !draft.trim()) return;
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

        <div ref={scrollRef} className="plain-thread chat-text">
          {empty ? (
            <div className="plain-welcome">
              <div className="plain-welcome-mascot">
                <img className="plain-welcome-logo" src={logo} alt="" />
                <span className="plain-welcome-cloud" aria-hidden>
                  <Cloud />
                  <RoleIcon size={22} />
                </span>
              </div>
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
            <div ref={contentRef}>
              <Transcript turn={chat.turn} onDecide={chat.decide} speaker={roleName} />
              {chat.error && <div className="plain-error">{chat.error}</div>}
            </div>
          )}
        </div>
      </section>

      <div className="plain-composer-wrap">
        {/* On the box's top edge, where the agent's says which folder: here, who the
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
                {roleName}
              </span>
            }
            value={chat.role}
            options={chat.roles.map((r) => ({ value: r.id, label: r.name, hint: r.description }))}
            onPick={(value) => chat.setRole(value as ChatRoleId)}
          />
          {chat.role === "kubernetes" && (
            <Dropdown
              title="The kubeconfig this chat works with"
              heading="Kubeconfig"
              label={
                <span className={`plain-chip-label${config ? "" : " unset"}`}>
                  <FileCog size={13} />
                  {config?.name ?? "No kubeconfig"}
                </span>
              }
              value={config?.name ?? ""}
              options={[
                ...kube.configs.map((c) => ({ value: c.name, hint: c.path })),
                { value: MANAGE_KUBECONFIGS, label: kube.configs.length ? "Manage kubeconfigs…" : "Add a kubeconfig…" },
              ]}
              onPick={(value) => {
                if (value === MANAGE_KUBECONFIGS) return onSetUpKube();
                chat.setPin({ kubeconfig: value, context: null, namespace: null });
                // Where the next new chat starts.
                void kube.pick(value);
              }}
            />
          )}
          {chat.role === "kubernetes" && config && chat.kube && (
            <Dropdown
              title="The context of the kubeconfig — which cluster, as which user"
              heading="Context"
              label={
                <span className={`plain-chip-label${target.context ? "" : " unset"}`}>
                  <Server size={13} />
                  {target.context?.name ?? chat.kube.context ?? "No context"}
                </span>
              }
              value={target.context?.name ?? ""}
              options={(target.contexts?.contexts ?? []).map((c) => ({
                value: c.name,
                hint: [c.cluster, c.namespace, c.name === target.contexts?.current ? "current" : null].filter(Boolean).join(" · "),
              }))}
              emptyLabel={target.error ?? "The kubeconfig has no contexts"}
              onPick={(context) => chat.kube && chat.setPin({ ...chat.kube, context, namespace: null })}
            />
          )}
          {chat.role === "kubernetes" && config && chat.kube && target.context && (
            <Dropdown
              title="The namespace this chat works in"
              heading="Namespace"
              label={
                <span className="plain-chip-label">
                  <Layers size={13} />
                  {target.namespace}
                </span>
              }
              value={target.namespace}
              options={namespaceOptions(target.namespace, target.namespaces)}
              onOpen={target.loadNamespaces}
              note={
                target.namespaces === null
                  ? "Asking the cluster…"
                  : target.namespaces.clusterError
                    ? `The cluster did not list its namespaces: ${target.namespaces.clusterError}`
                    : undefined
              }
              custom={{
                placeholder: "Another namespace",
                onEnter: (namespace) => {
                  if (!chat.kube) return;
                  chat.setPin({ ...chat.kube, namespace });
                  void rememberNamespace(chat.kube.kubeconfig, namespace).catch(() => {});
                },
              }}
              onPick={(namespace) => chat.kube && chat.setPin({ ...chat.kube, namespace })}
            />
          )}
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
            {chat.context && (
              <span className="plain-meter">
                <ContextMeter context={chat.context} usage={chat.turn.usage} running={chat.busy} onCompact={onCompact} up />
              </span>
            )}
            {running ? (
              <button type="button" className="plain-send stop" title="Stop" onClick={chat.stop}>
                <Square size={14} />
              </button>
            ) : (
              <button type="button" className="plain-send" title="Send" disabled={chat.busy || !draft.trim()} onClick={send}>
                <SendHorizontal size={16} />
              </button>
            )}
          </div>
        </section>
      </div>
    </>
  );
}
