import { useEffect, useRef, useState, type ReactNode } from "react";
import { Bot, Brain, ChevronDown, Cpu, Database } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { Switch } from "./Switch";
import { matches } from "../lib/shortcuts";
import { CONVERSATION_MODES, type ConversationMode } from "../lib/chat";
import { choiceKey, type ModelChoice } from "../hooks/useLlmSettings";
import { effortOptions } from "../lib/providerForm";
import "./ComposerSettings.css";

/** A row of a panel: for a `Dropdown`'s `label`, stretched across the panel. */
export const panelRow = (icon: ReactNode, name: string, value: ReactNode) => (
  <span className="composer-settings-row-label">
    {icon}
    <span className="composer-settings-name">{name}</span>
    <span className="composer-settings-value">{value}</span>
  </span>
);

export const PanelLine = () => <div className="composer-settings-line" />;

type SwitchRow = {
  label: string;
  icon: ReactNode;
  title: string;
  on: boolean;
  onChange: (on: boolean) => void;
  /** On, this is a state not to be missed: its icon says so in amber. */
  warn?: boolean;
};

export function PanelSwitch({ label, icon, title, on, onChange, warn }: SwitchRow) {
  return (
    <label className={`composer-settings-switch${on && warn ? " warn" : ""}`} title={title}>
      {icon}
      <span className="composer-settings-name">{label}</span>
      <Switch label={label} on={on} onChange={onChange} />
    </label>
  );
}

/**
 * Settings said in one line under the box and changed in a panel over it. The
 * button has no position of its own, so the panel hangs from the box's wrapper:
 * above the box and what waits on it, neither of which it covers.
 */
export function ComposerPanel({
  title,
  summary,
  className,
  left,
  children,
}: {
  title: string;
  /** The words on the button, a span each. */
  summary: ReactNode;
  className: string;
  /** Hangs from the box's left corner, for a button on the left of the line. */
  left?: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    // A menu open in the panel takes this Escape; the panel takes the next.
    const onKey = (e: KeyboardEvent) => {
      if (matches(e, "close") && !e.defaultPrevented) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className={className} ref={wrap}>
      <button
        type="button"
        className={`composer-settings-toggle${open ? " open" : ""}`}
        title={title}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span className="composer-settings-summary">{summary}</span>
        <ChevronDown className="composer-settings-chev" size={10} />
      </button>
      {open && (
        <div className={`composer-settings-panel${left ? " left" : ""}`} role="dialog" aria-label={title}>
          {children}
        </div>
      )}
    </div>
  );
}

type Props = {
  /** Every configured `provider/model`, the one turns go to now, and its thinking level. */
  models: { choices: ModelChoice[]; current: ModelChoice | null; effort?: string | null };
  onModel: (choice: ModelChoice) => void;
  /** `null` is the model's default — nothing sent. */
  onEffort: (effort: string | null) => void;
  /** Asks the providers what they serve; called when the model menu opens. */
  onLoadModels: () => void;
  /** What the agent is for; a plain chat has no such question. */
  mode?: { value: ConversationMode; onPick: (mode: ConversationMode) => void };
  /** Under the line: what is simply on or off. */
  toggles?: SwitchRow[];
  /** A span said last on the line under the box. */
  note?: ReactNode;
  /** The folder's index, at the panel's end: read, not set. */
  status?: ReactNode;
};

/** What the next message goes out with: the mode, the model, its thinking level. */
export function ComposerSettings({ models, onModel, onEffort, onLoadModels, mode, toggles = [], note, status }: Props) {
  const effort = models.effort ?? "";
  const efforts = effortOptions(effort);
  const effortLabel = efforts.find((e) => e.value === effort)?.label;
  const modeLabel = CONVERSATION_MODES.find((c) => c.value === mode?.value)?.label;
  // The provider is read in the panel; the line has room for the model alone.
  const label = models.current?.label ?? "no model";
  const cut = label.lastIndexOf("/") + 1;

  return (
    <ComposerPanel
      title="Mode, model and thinking"
      className="composer-settings"
      summary={
        <>
          {modeLabel && <span>{modeLabel.toLowerCase()}</span>}
          <span className="composer-settings-shrink">{label.slice(cut)}</span>
          {models.current && <span>{effortLabel?.toLowerCase()}</span>}
          {note}
        </>
      }
    >
      {/* Two questions, not one: this row is what the agent is for; the
          switch under the line is who has to agree before it acts — an
          Analyst turn still asks before a server's tool, and an unattended
          agent still cannot edit a file there. */}
      {mode && (
        <Dropdown
          right
          title="What the agent may do"
          heading="Mode"
          label={panelRow(<Bot size={14} />, "Mode", modeLabel ?? "Agent")}
          value={mode.value}
          options={CONVERSATION_MODES.map((c) => ({ value: c.value, label: c.label, hint: c.hint }))}
          onPick={(v) => mode.onPick(v as ConversationMode)}
        />
      )}
      <Dropdown
        right
        title="Model"
        heading="Model"
        label={panelRow(
          <Cpu size={14} />,
          "Model",
          <>
            <span className="composer-settings-provider">{label.slice(0, cut)}</span>
            <span className="composer-settings-model">{label.slice(cut)}</span>
          </>,
        )}
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
          right
          title="Thinking level"
          heading="Thinking"
          label={panelRow(<Brain size={14} />, "Thinking", effortLabel)}
          value={effort}
          options={efforts}
          onPick={(v) => onEffort(v || null)}
        />
      )}
      {toggles.length > 0 && <PanelLine />}
      {toggles.map((toggle) => (
        <PanelSwitch key={toggle.label} {...toggle} />
      ))}
      {status && (
        <div className="composer-settings-status">
          <Database size={14} />
          <span className="composer-settings-name">Index</span>
          {status}
        </div>
      )}
    </ComposerPanel>
  );
}
