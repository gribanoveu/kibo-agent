import { useState, type ComponentProps } from "react";
import { Bell, Bot, Check, Languages, Palette, Shield, ShieldCheck, Sparkles } from "lucide-react";
import { ProviderSettings } from "./ProviderSettings";
import { DataPolicy } from "./DataPolicy";
import { ItemList } from "./ItemList";
import type { RememberScope, ReplyLanguage, SkillSourceItem, SkillsView } from "../lib/chat";
import type { PanelItem } from "../types";
import { MODES, sideOf, THEME_LABELS, themesOf, type Mode, type Theme, type ThemeChoice } from "../hooks/useTheme";
import { FONT_SIZES, type FontSize } from "../hooks/useChatFontSize";
import type { AlertPrefs, Attention } from "../lib/attention";
import "./Settings.css";

// The settings dialog: one subject per section, picked on the left, so the
// provider form is not scrolled past to reach the theme.

const SECTIONS = [
  { id: "models", label: "Models", icon: Bot },
  { id: "language", label: "Language", icon: Languages },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "notifications", label: "Notifications", icon: Bell },
  { id: "skills", label: "Skills", icon: Sparkles },
  { id: "permissions", label: "Permissions", icon: ShieldCheck },
  { id: "privacy", label: "Privacy", icon: Shield },
] as const;

type Section = (typeof SECTIONS)[number]["id"];

const REMEMBER: { value: RememberScope; label: string; hint: string }[] = [
  {
    value: "chat",
    label: "Per chat",
    hint: "Each chat keeps the Ask or Auto it was left in. A new chat starts in Ask.",
  },
  {
    value: "repository",
    label: "Per repository",
    hint: "One choice for every chat in the open folder, new ones included. Other folders keep their own.",
  },
];

const LANGUAGES: { value: ReplyLanguage; label: string; hint: string }[] = [
  {
    value: "auto",
    label: "Auto",
    hint: "The language you write in. /review has nothing you wrote to go by and answers in English.",
  },
  { value: "english", label: "English", hint: "Every answer, plan and review in English, whatever you write in." },
  { value: "russian", label: "Русский", hint: "Every answer, plan and review in Russian, whatever you write in. Code stays as it is." },
];

/** What each skills folder is called here, and what reading it means. */
const SOURCES: Record<SkillSourceItem["id"], { badge: string; title: string; desc: string }> = {
  project: {
    badge: "PR",
    title: "The repository's",
    desc: ".claude/skills and .agents/skills, from the open folder up to the git root",
  },
  app: { badge: "KI", title: "Kibo's", desc: "This app's own skills folder" },
  agents: { badge: "AG", title: "Codex and other agents'", desc: "Where skill installers put them" },
  claude: { badge: "CL", title: "Claude Code's", desc: "Your personal Claude Code skills" },
};

function sourceItems(sources: SkillSourceItem[]): PanelItem[] {
  return sources.map((source) => ({
    id: source.id,
    kind: "skill",
    ...SOURCES[source.id],
    enabled: source.enabled,
    meta: source.path || "No folder open",
  }));
}

type Props = {
  provider: ComponentProps<typeof ProviderSettings>;
  /** The skills folders: which of them are read at all. */
  skills: { view: SkillsView | null; error: string | null; onToggle: (id: SkillSourceItem["id"], enabled: boolean) => void };
  /** Where the composer's Ask/Auto is remembered. */
  remember: RememberScope;
  onRemember: (remember: RememberScope) => void;
  debugLogging: boolean;
  onDebugLogging: (enabled: boolean) => void;
  /** What the model writes its replies in. */
  replyLanguage: ReplyLanguage;
  onReplyLanguage: (language: ReplyLanguage) => void;
  theme: ThemeChoice;
  onThemeMode: (mode: Mode) => void;
  onThemePalette: (theme: Theme) => void;
  fontSize: FontSize;
  onFontSize: (size: FontSize) => void;
  /** The file viewer's long lines: wrapped, or scrolled sideways. */
  wrapLines: boolean;
  onWrapLines: (wrap: boolean) => void;
  /** Which moments call the user back, by sound and by system notification. */
  alerts: AlertPrefs;
  onAlerts: (next: AlertPrefs) => void;
  onOpenLog: () => void;
  policy: ComponentProps<typeof DataPolicy>;
};

export function Settings({
  provider,
  skills,
  remember,
  onRemember,
  debugLogging,
  onDebugLogging,
  replyLanguage,
  onReplyLanguage,
  theme,
  onThemeMode,
  onThemePalette,
  fontSize,
  onFontSize,
  wrapLines,
  onWrapLines,
  alerts,
  onAlerts,
  onOpenLog,
  policy,
}: Props) {
  const [section, setSection] = useState<Section>("models");

  return (
    <div className="settings">
      <nav className="settings-nav" aria-label="Settings sections">
        {SECTIONS.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            type="button"
            className={`settings-nav-item${section === id ? " active" : ""}`}
            aria-current={section === id ? "page" : undefined}
            onClick={() => setSection(id)}
          >
            <Icon size={14} />
            {label}
          </button>
        ))}
      </nav>

      <div className="settings-pane">
        {section === "models" && (
          <>
            <h3 className="settings-title">Model provider</h3>
            <ProviderSettings {...provider} />
          </>
        )}

        {section === "language" && (
          <>
            <h3 className="settings-title">Language</h3>
            <div className="modal-field">
              <label>Reply language</label>
              <div className="segmented" role="radiogroup" aria-label="Reply language">
                {LANGUAGES.map(({ value, label }) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={replyLanguage === value}
                    className={`segment${replyLanguage === value ? " active" : ""}`}
                    onClick={() => onReplyLanguage(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <p className="modal-note settings-hint">{LANGUAGES.find((l) => l.value === replyLanguage)?.hint}</p>
          </>
        )}

        {section === "appearance" && (
          <>
            <h3 className="settings-title">Appearance</h3>
            <div className="modal-field">
              <label>Theme</label>
              <div className="segmented" role="radiogroup" aria-label="Theme">
                {MODES.map((mode) => (
                  <button
                    key={mode}
                    type="button"
                    role="radio"
                    aria-checked={theme.mode === mode}
                    className={`segment${theme.mode === mode ? " active" : ""}`}
                    onClick={() => onThemeMode(mode)}
                  >
                    {MODE_LABELS[mode]}
                  </button>
                ))}
              </div>
            </div>
            <p className="modal-note settings-hint">
              {theme.mode === "system"
                ? `As the system is set: ${THEME_LABELS[theme.light]} by day, ${THEME_LABELS[theme.dark]} at night.`
                : `Always ${THEME_LABELS[theme[theme.mode]]}, whatever the system is set to.`}
            </p>
            {(["light", "dark"] as const).map((side) => (
              <section key={side} className="theme-group">
                <h4 className="theme-group-title">
                  {side === "light" ? "Light theme" : "Dark theme"}
                  {sideOf(theme) === side && <span className="theme-group-badge">showing</span>}
                </h4>
                <div className="theme-cards" role="radiogroup" aria-label={side === "light" ? "Light theme" : "Dark theme"}>
                  {themesOf(side).map((name) => (
                    <button
                      key={name}
                      type="button"
                      role="radio"
                      aria-checked={theme[side] === name}
                      className={`theme-card${theme[side] === name ? " active" : ""}`}
                      onClick={() => onThemePalette(name)}
                    >
                      <ThemePreview theme={name} />
                      <span className="theme-card-name">
                        {theme[side] === name && <Check size={13} aria-hidden />}
                        {THEME_LABELS[name]}
                      </span>
                    </button>
                  ))}
                </div>
              </section>
            ))}
            <div className="modal-field">
              <label>Chat text size</label>
              <div className="segmented" role="radiogroup" aria-label="Chat text size">
                {(Object.keys(FONT_SIZES) as FontSize[]).map((name) => (
                  <button
                    key={name}
                    type="button"
                    role="radio"
                    aria-checked={fontSize === name}
                    className={`segment${fontSize === name ? " active" : ""}`}
                    onClick={() => onFontSize(name)}
                  >
                    {name}
                  </button>
                ))}
              </div>
            </div>
            <div className="modal-field">
              <label>Long lines in the file viewer</label>
              <div className="segmented" role="radiogroup" aria-label="Long lines in the file viewer">
                {(
                  [
                    ["wrap", true],
                    ["scroll sideways", false],
                  ] as const
                ).map(([label, value]) => (
                  <button
                    key={label}
                    type="button"
                    role="radio"
                    aria-checked={wrapLines === value}
                    className={`segment${wrapLines === value ? " active" : ""}`}
                    onClick={() => onWrapLines(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
          </>
        )}

        {section === "notifications" && (
          <>
            <h3 className="settings-title">Notifications</h3>
            <table className="alert-table">
              <thead>
                <tr>
                  <th />
                  <th scope="col">Sound</th>
                  <th scope="col">Notification</th>
                </tr>
              </thead>
              <tbody>
                {ALERT_ROWS.map(({ attention, label }) => (
                  <tr key={attention}>
                    <th scope="row">{label}</th>
                    <td>
                      {attention === "approval" ? (
                        <span className="alert-none" title="In Ask a card comes with every step that writes">
                          —
                        </span>
                      ) : (
                        <AlertSwitch
                          label={`${label}: sound`}
                          on={alerts.sound[attention]}
                          onChange={(on) => onAlerts({ ...alerts, sound: { ...alerts.sound, [attention]: on } })}
                        />
                      )}
                    </td>
                    <td>
                      <AlertSwitch
                        label={`${label}: notification`}
                        on={alerts.notify[attention]}
                        onChange={(on) => onAlerts({ ...alerts, notify: { ...alerts.notify, [attention]: on } })}
                      />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className="modal-note settings-list-note">
              A card has no sound: in Ask one comes with every step that writes. Notifications come only while Kibo is
              in the background; the system&apos;s notification settings can silence them all.
            </p>
          </>
        )}

        {section === "skills" && (
          <>
            <h3 className="settings-title">Skills</h3>
            <ItemList
              label="Where skills come from"
              count={`${(skills.view?.sources ?? []).filter((s) => s.enabled).length}/${skills.view?.sources.length ?? 0}`}
              items={sourceItems(skills.view?.sources ?? [])}
              emptyLabel="Reading the folders…"
              onToggle={(id, enabled) => skills.onToggle(id as SkillSourceItem["id"], enabled)}
            />
            {skills.error && <p className="modal-note settings-error">{skills.error}</p>}
            <p className="modal-note settings-list-note">
              A folder switched off is not read at all, in any repository. When two folders have a skill of the same
              name, the one higher in this list is used. Single skills are switched off in the Skills panel.
            </p>
          </>
        )}

        {section === "permissions" && (
          <>
            <h3 className="settings-title">Permissions</h3>
            <div className="modal-field">
              <label>Remember Ask / Auto</label>
              <div className="segmented" role="radiogroup" aria-label="Remember Ask / Auto">
                {REMEMBER.map(({ value, label }) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={remember === value}
                    className={`segment${remember === value ? " active" : ""}`}
                    onClick={() => onRemember(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <p className="modal-note settings-hint">{REMEMBER.find((r) => r.value === remember)?.hint}</p>
          </>
        )}

        {section === "privacy" && (
          <>
            <h3 className="settings-title">Privacy</h3>
            <div className="modal-field">
              <label>Request log</label>
              <div className="segmented" role="radiogroup" aria-label="Request log">
                {[
                  ["off", false],
                  ["on", true],
                ].map(([label, value]) => (
                  <button
                    key={String(label)}
                    type="button"
                    role="radio"
                    aria-checked={debugLogging === value}
                    className={`segment${debugLogging === value ? " active" : ""}`}
                    onClick={() => onDebugLogging(Boolean(value))}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
            <p className="modal-note settings-hint">
              Writes whole conversations, including every file the agent read, to a file in your home directory.
            </p>
            <div className="modal-field">
              <label>Tool calls</label>
              <div>
                <button className="btn btn-ghost" type="button" onClick={onOpenLog}>
                  Open the log
                </button>
              </div>
            </div>
            <DataPolicy {...policy} />
          </>
        )}
      </div>
    </div>
  );
}

const ALERT_ROWS: { attention: Attention; label: string }[] = [
  { attention: "done", label: "Finished" },
  { attention: "failed", label: "Failed" },
  { attention: "approval", label: "Waiting for approval" },
];

function AlertSwitch({ label, on, onChange }: { label: string; on: boolean; onChange: (on: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      className={`alert-switch${on ? " on" : ""}`}
      onClick={() => onChange(!on)}
    />
  );
}

const MODE_LABELS: Record<Mode, string> = { system: "System", light: "Light", dark: "Dark" };

/**
 * A few of the app's own elements — a message, an answer with code and a
 * link, a changed line, a button — in `theme`'s palette, drawn with the classes
 * the chat and the diff use, so a card shows what the app will look like
 * rather than a picture of it. The palette comes from `data-theme` on it.
 */
function ThemePreview({ theme }: { theme: Theme }) {
  return (
    <span className="theme-preview" data-theme={theme} aria-hidden>
      <span className="theme-preview-body">
        <span className="bubble theme-preview-ask">Why does the index rebuild?</span>
        <span className="theme-preview-answer">
          The watcher reports the path, and <code className="md-code-inline">index_sync.rs</code> updates the
          keywords. <span className="md-link">Notes</span>
        </span>
        <span className="theme-preview-diff">
          <span className="diff-row diff-del">
            <span className="diff-sign">-</span>
            <span className="diff-text">
              let <mark>old</mark> = watch(root);
            </span>
          </span>
          <span className="diff-row diff-add">
            <span className="diff-sign">+</span>
            <span className="diff-text">
              let <mark>watcher</mark> = watch(root);
            </span>
          </span>
        </span>
        <span className="theme-preview-actions">
          <span className="btn btn-primary">Implement</span>
          <span className="btn btn-ghost">Review</span>
        </span>
      </span>
    </span>
  );
}
