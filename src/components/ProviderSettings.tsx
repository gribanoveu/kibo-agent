import { useEffect, useRef, useState } from "react";
import { Check, Circle, CircleDot, FileUp, Plus, RefreshCw, Search, X } from "lucide-react";
import type { KeyStore, LlmSettings, ProviderConfig, ProviderKind } from "../lib/chat";
import { certificateCount, EFFORTS, filterModels, pemFromBytes } from "../lib/providerForm";
import "./ProviderSettings.css";

// The provider form. Segmented controls rather than the app's dropdown,
// because a popup menu is clipped by the modal's own overflow — the same
// reason the theme picker is segmented.

/** Mirrors `domain::settings::DEFAULT_CONTEXT_LIMIT`: what a provider that never set one gets. */
export const DEFAULT_CONTEXT_LIMIT = 260_000;

/** Past this many models the list gets a search box. */
const SEARCH_FROM = 8;

const BLANK = {
  id: "",
  kind: "openAiCompatible" as ProviderKind,
  baseUrl: "",
  modelsUrl: "",
  model: "",
  contextLimit: String(DEFAULT_CONTEXT_LIMIT),
  reasoningEffort: "",
  // "" is "not set": nothing is sent, and the model uses its own default.
  temperature: "",
  topP: "",
  cert: "",
};

type Draft = typeof BLANK;

const KINDS: [ProviderKind, string, string][] = [
  ["openAiCompatible", "OpenAI-compatible", "https://api.openai.com/v1"],
  ["anthropic", "Anthropic", "https://api.anthropic.com/v1"],
];

type Props = {
  settings: LlmSettings | null;
  busy: boolean;
  error: string | null;
  /** Resolves to whether it was stored — the form says so rather than making it a guess. */
  onSave: (provider: ProviderConfig, apiKey: string | null) => void | Promise<boolean>;
  onRemove: (id: string) => void;
  onSelect: (id: string) => void;
  /** Moves the key the API keys are sealed under between `~/.kibo` and the system keychain. */
  onKeyStore?: (store: KeyStore) => void;
  /** Asks the provider, as the form has it now, what it serves. */
  onProbe?: (provider: ProviderConfig, apiKey: string | null) => Promise<string[]>;
  /** What each provider was last seen to serve, by id. */
  served?: Record<string, string[]>;
};

/** A stored f32 comes back as 0.699999988; the form shows what was typed. */
const sampling = (value: number | null | undefined) =>
  value == null ? "" : String(Math.round(value * 100) / 100);

const fromConfig = (config: ProviderConfig): Draft => ({
  id: config.id,
  kind: config.kind ?? "openAiCompatible",
  baseUrl: config.baseUrl,
  modelsUrl: config.modelsUrl ?? "",
  model: config.model ?? "",
  contextLimit: String(config.contextLimit || DEFAULT_CONTEXT_LIMIT),
  reasoningEffort: config.reasoningEffort ?? "",
  temperature: sampling(config.temperature),
  topP: sampling(config.topP),
  cert: config.trustedCertPem ?? "",
});

export function ProviderSettings({
  settings,
  busy,
  error,
  onSave,
  onRemove,
  onSelect,
  onKeyStore,
  onProbe,
  served = {},
}: Props) {
  const providers = settings?.providers ?? [];
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft>(BLANK);
  // `null` means the field was never touched, which is not the same as an
  // empty one: an empty one deletes the stored key.
  const [apiKey, setApiKey] = useState<string | null>(null);
  // Saving a key gives nothing back to look at — the field empties either way,
  // and a key that silently did not arrive is found out a turn later.
  const [saved, setSaved] = useState(false);
  // The last ask for a list, by the name it was asked under: a switch to
  // another provider must not show this one's spinner or refusal.
  const [listing, setListing] = useState<{ id: string; loading: boolean; error: string | null } | null>(null);

  // Follow the backend rather than remember a stale copy of it: a save that
  // renamed or removed a provider must not leave the form editing a ghost.
  useEffect(() => {
    if (!settings) return;
    const active = settings.activeProviderId ?? providers[0]?.id ?? null;
    setEditing(active);
    const chosen = providers.find((p) => p.id === active);
    setDraft(chosen ? fromConfig(chosen) : BLANK);
    setApiKey(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings]);

  const chosen = providers.find((p) => p.id === editing);
  const pick = (id: string | null) => {
    setEditing(id);
    setApiKey(null);
    setSaved(false);
    const next = providers.find((p) => p.id === id);
    setDraft(next ? fromConfig(next) : BLANK);
    if (id) onSelect(id);
  };

  const config = (): ProviderConfig => ({
    ...chosen,
    id: draft.id.trim(),
    kind: draft.kind,
    baseUrl: draft.baseUrl.trim(),
    // Blank is the default path, built from the base URL.
    modelsUrl: draft.modelsUrl.trim() || null,
    model: draft.model.trim() || null,
    // Blank, zero or a typo is "not set", which the backend reads as the
    // default window rather than as a window of nothing.
    contextLimit: Number.parseInt(draft.contextLimit, 10) || null,
    // Blank sends nothing and leaves the model's own default.
    reasoningEffort: draft.reasoningEffort.trim() || null,
    temperature: draft.temperature === "" ? null : Number(draft.temperature),
    topP: draft.topP === "" ? null : Number(draft.topP),
    trustedCertPem: draft.cert.trim() || null,
  });

  const save = async (e?: React.FormEvent) => {
    // The form is here for one reason: Enter. Every other box in this app
    // sends on Enter, and a settings box that quietly does nothing looks
    // exactly like a settings box that saved.
    e?.preventDefault();
    const ok = await onSave(config(), apiKey);
    setSaved(ok !== false);
  };

  const edit = (next: Partial<Draft>) => {
    setDraft((prev) => ({ ...prev, ...next }));
    setSaved(false);
  };

  const models = served[draft.id.trim()] ?? null;
  const canList = !!onProbe && !!draft.id.trim() && !!draft.baseUrl.trim();
  // The form as it stands, unless told otherwise: the effect below runs
  // before a newly picked provider's values reach `draft`.
  const list = async (provider = config(), key = apiKey) => {
    if (!onProbe || !provider.id || !provider.baseUrl) return;
    const id = provider.id;
    setListing({ id, loading: true, error: null });
    try {
      await onProbe(provider, key);
      setListing({ id, loading: false, error: null });
    } catch (e) {
      setListing({ id, loading: false, error: String(e) });
    }
  };
  const status = listing?.id === draft.id.trim() ? listing : null;

  // A stored provider with a key is asked once when it is opened, so its
  // list is there without a click. Not a new one: its URL is half-typed.
  useEffect(() => {
    if (chosen?.hasApiKey && !served[chosen.id]) list(chosen, null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chosen?.id]);

  return (
    <div className="provider-settings">
      {providers.length > 0 && (
        <div className="provider-tabs" role="radiogroup" aria-label="Provider">
          {providers.map((provider) => (
            <button
              key={provider.id}
              type="button"
              role="radio"
              aria-checked={provider.id === editing}
              className={`provider-tab${provider.id === editing ? " active" : ""}`}
              onClick={() => pick(provider.id)}
            >
              {provider.id}
            </button>
          ))}
          <button
            type="button"
            className={`provider-tab provider-tab-add${editing === null ? " active" : ""}`}
            onClick={() => pick(null)}
            title="Add another provider"
          >
            <Plus size={12} />
            Add
          </button>
        </div>
      )}

      <form
        onSubmit={save}
        // The checks are the backend's. A browser's own would block the
        // submit silently — a slider's step read as a mismatch did.
        noValidate
        // Enter in any of the boxes, stated rather than left to the browser's
        // implicit submission — which a webview can decline, and which is
        // indistinguishable from a settings panel that does not save.
        onKeyDown={(e) => {
          if (e.key !== "Enter" || !(e.target instanceof HTMLInputElement)) return;
          e.preventDefault();
          save();
        }}
      >
        <section className="provider-card">
          <h4 className="provider-card-title">Connection</h4>
          <div className="modal-field">
            <label>Name</label>
            <input type="text" value={draft.id} placeholder="openai" onChange={(e) => edit({ id: e.target.value })} />
          </div>
          <div className="modal-field">
            <label>Protocol</label>
            <div className="segmented" role="radiogroup" aria-label="Protocol">
              {KINDS.map(([kind, label]) => (
                <button
                  key={kind}
                  type="button"
                  role="radio"
                  aria-checked={draft.kind === kind}
                  className={`segment${draft.kind === kind ? " active" : ""}`}
                  onClick={() => edit({ kind })}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
          <div className="modal-field">
            <label>Base URL</label>
            <input
              type="text"
              value={draft.baseUrl}
              placeholder={KINDS.find(([kind]) => kind === draft.kind)?.[2]}
              onChange={(e) => edit({ baseUrl: e.target.value })}
            />
          </div>
          <div className="modal-field">
            <label>Models URL</label>
            <input
              type="text"
              value={draft.modelsUrl}
              // The path it takes when left blank, spelled out: that is what
              // someone correcting it starts from.
              placeholder={draft.baseUrl.trim() ? `${draft.baseUrl.trim().replace(/\/+$/, "")}/models` : "base URL + /models"}
              onChange={(e) => edit({ modelsUrl: e.target.value })}
            />
          </div>
          <div className="modal-field">
            <label>API key</label>
            <input
              type="password"
              value={apiKey ?? ""}
              placeholder={chosen?.hasApiKey ? "stored — type to replace" : "not configured"}
              onChange={(e) => {
                setApiKey(e.target.value);
                setSaved(false);
              }}
              // A key typed and left is stored at once, with the provider it
              // belongs to: the list refresh and the composer's model menu both
              // need it, and a Save not yet pressed is not something to fail on.
              // Not a cleared field — that deletes the stored key, and only Save
              // should do it.
              onBlur={() => {
                if (apiKey?.trim() && draft.id.trim() && draft.baseUrl.trim()) save();
              }}
            />
          </div>
          <KeyStorage store={settings?.keyStore ?? "file"} busy={busy} onMove={onKeyStore} />
        </section>

        <section className="provider-card">
          <div className="provider-card-head">
            <h4 className="provider-card-title">
              Models
              {models && models.length > 0 && <span className="provider-count">{models.length}</span>}
            </h4>
            <button
              type="button"
              className="btn btn-ghost provider-refresh"
              disabled={!canList || status?.loading}
              onClick={() => list()}
              title={canList ? "Ask the provider what it serves" : "Needs a name and a base URL"}
            >
              <RefreshCw size={12} className={status?.loading ? "spinning" : undefined} />
              {models ? "Refresh" : "Load list"}
            </button>
          </div>
          {status?.error && <p className="modal-note provider-error">{status.error}</p>}
          {models && models.length > 0 ? (
            <ModelList models={models} current={draft.model.trim()} onPick={(model) => edit({ model })} />
          ) : (
            <div className="modal-field">
              <label>Model</label>
              <input
                type="text"
                value={draft.model}
                placeholder="auto — the first one the provider lists"
                onChange={(e) => edit({ model: e.target.value })}
              />
            </div>
          )}
        </section>

        <section className="provider-card">
          <h4 className="provider-card-title">Generation</h4>
          <Slider
            label="Temperature"
            value={draft.temperature}
            max={2}
            onChange={(temperature) => edit({ temperature })}
          />
          <Slider label="Top P" value={draft.topP} max={1} onChange={(topP) => edit({ topP })} />
          {draft.kind === "anthropic" && draft.temperature !== "" && draft.topP !== "" && (
            <p className="modal-note settings-hint provider-warn">
              Newer Claude models refuse a request that sets both — keep one at the default.
            </p>
          )}
          <div className="modal-field">
            <label>Reasoning effort</label>
            <div className="segmented" role="radiogroup" aria-label="Reasoning effort">
              {EFFORTS.map(({ value, label }) => (
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={draft.reasoningEffort.trim() === value}
                  className={`segment${draft.reasoningEffort.trim() === value ? " active" : ""}`}
                  onClick={() => edit({ reasoningEffort: value })}
                >
                  {label}
                </button>
              ))}
            </div>
            <div className="provider-effort-custom">
              <span>Custom</span>
              <input
                type="text"
                value={draft.reasoningEffort}
                placeholder={
                  draft.kind === "anthropic" ? "max, or a token budget for older models" : "minimal, xhigh…"
                }
                onChange={(e) => edit({ reasoningEffort: e.target.value })}
              />
            </div>
          </div>
          <div className="modal-field">
            <label>Context window</label>
            <input
              type="text"
              inputMode="numeric"
              value={draft.contextLimit}
              placeholder={`${DEFAULT_CONTEXT_LIMIT} tokens`}
              onChange={(e) => edit({ contextLimit: e.target.value.replace(/\D/g, "") })}
            />
          </div>
          <p className="modal-note settings-hint">
            In tokens. The older part of a chat is folded into a summary as it nears this.
          </p>
        </section>

        <Certificate pem={draft.cert} onChange={(cert) => edit({ cert })} />

        <div className="provider-foot">
          <button className="btn btn-primary" type="submit" disabled={busy}>
            Save
          </button>
          {chosen && (
            <button
              className="btn btn-ghost provider-remove"
              type="button"
              disabled={busy}
              onClick={() => onRemove(chosen.id)}
            >
              Remove
            </button>
          )}
          {error && <span className="modal-note provider-error">{error}</span>}
          {saved && !error && (
            <span className="modal-note provider-saved">
              <Check size={12} />
              Saved.
            </span>
          )}
        </div>
      </form>
    </div>
  );
}

/**
 * Where the keys are kept, and the offer to move them into the keychain.
 * Opt-in: the keychain asks the user for access, which on a first launch reads
 * as distrust — and after every update, for an ad-hoc signed build.
 */
function KeyStorage({
  store,
  busy,
  onMove,
}: {
  store: KeyStore;
  busy: boolean;
  onMove?: (store: KeyStore) => void;
}) {
  const inKeychain = store === "keychain";
  return (
    <div className="provider-key-store">
      <p className="modal-note">
        {inKeychain
          ? "Keys are sealed on disk, under a key held in the system keychain."
          : "Keys are sealed in ~/.kibo, readable only by your account. The system keychain also keeps them out of backups and synced folders."}
      </p>
      {onMove && (
        <button
          type="button"
          className="btn btn-ghost"
          disabled={busy}
          title={inKeychain ? undefined : "The system will ask to allow access"}
          onClick={() => onMove(inKeychain ? "file" : "keychain")}
        >
          {inKeychain ? "Move to ~/.kibo" : "Move to keychain"}
        </button>
      )}
    </div>
  );
}

/**
 * The models to choose the default from. "Auto" leaves it to the provider's
 * first; a name the list does not have can still be typed into the search.
 */
function ModelList({
  models,
  current,
  onPick,
}: {
  models: string[];
  current: string;
  onPick: (model: string) => void;
}) {
  const [query, setQuery] = useState("");
  const shown = filterModels(models, query);
  const typed = query.trim();
  // The default stays in view even when the provider no longer lists it, or
  // the search hides it: it is what the next turn will send.
  const pinned = current && !shown.includes(current) ? [current] : [];

  const row = (model: string, label = model, hint?: string) => (
    <button
      key={`m:${model}`}
      type="button"
      role="radio"
      aria-checked={model === current}
      className={`model-row${model === current ? " active" : ""}`}
      onClick={() => onPick(model)}
    >
      {model === current ? (
        <CircleDot className="model-radio" size={15} aria-hidden />
      ) : (
        <Circle className="model-radio" size={15} aria-hidden />
      )}
      <span className="model-name">{label}</span>
      {hint && <span className="model-hint">{hint}</span>}
      {model === current && <span className="model-badge">default</span>}
    </button>
  );

  return (
    <div className="model-picker">
      {models.length > SEARCH_FROM && (
        <label className="model-search">
          <Search size={12} />
          <input
            type="search"
            value={query}
            placeholder={`Search ${models.length} models`}
            onChange={(e) => setQuery(e.target.value)}
            // Enter picks the first match rather than saving the form.
            onKeyDown={(e) => {
              if (e.key !== "Enter") return;
              e.preventDefault();
              e.stopPropagation();
              const first = shown[0] ?? typed;
              if (first) onPick(first);
            }}
          />
        </label>
      )}
      <div className="model-list" role="radiogroup" aria-label="Default model">
        {!typed && row("", "Auto", "the first one the provider lists")}
        {pinned.map((model) => row(model, model, "not listed"))}
        {shown.map((model) => row(model))}
        {typed && !models.includes(typed) && row(typed, `Use “${typed}”`)}
        {typed && !shown.length && <p className="model-empty">No model matches.</p>}
      </div>
    </div>
  );
}

/**
 * A sampling setting: a slider, or "default" — nothing sent, which is not the
 * same as any value on it.
 */
function Slider({
  label,
  value,
  max,
  onChange,
}: {
  label: string;
  value: string;
  max: number;
  onChange: (value: string) => void;
}) {
  const unset = value === "";
  return (
    <div className="modal-field">
      <label>{label}</label>
      <div className={`provider-slider${unset ? " unset" : ""}`}>
        <input
          type="range"
          aria-label={label}
          aria-valuetext={unset ? "model default" : value}
          min={0}
          max={max}
          step={0.05}
          value={unset ? max / 2 : value}
          style={{ "--fill": `${((unset ? 0 : Number(value)) / max) * 100}%` } as React.CSSProperties}
          onChange={(e) => onChange(String(Number(e.target.value)))}
        />
        <span className="provider-slider-value">{unset ? "default" : Number(value).toFixed(2)}</span>
        <button
          type="button"
          className="provider-slider-reset"
          disabled={unset}
          title="Use the model's default"
          aria-label={`Reset ${label}`}
          onClick={() => onChange("")}
        >
          <X size={12} />
        </button>
      </div>
    </div>
  );
}

/**
 * A certificate to trust for this provider: a gateway behind a corporate CA,
 * or a self-signed server. Loaded from a file or pasted; shown as a count, and
 * as text only when asked.
 */
function Certificate({ pem, onChange }: { pem: string; onChange: (pem: string) => void }) {
  const file = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const count = certificateCount(pem);

  const load = async (chosen: File | undefined) => {
    if (!chosen) return;
    try {
      onChange(pemFromBytes(new Uint8Array(await chosen.arrayBuffer())));
      setReadError(null);
    } catch (e) {
      setReadError(`${chosen.name} could not be read: ${e}`);
    }
  };

  return (
    <section className="provider-card">
      <div className="provider-card-head">
        <h4 className="provider-card-title">Certificate</h4>
        <div className="provider-cert-actions">
          <button type="button" className="btn btn-ghost" onClick={() => file.current?.click()}>
            <FileUp size={12} />
            {pem.trim() ? "Replace…" : "Load file…"}
          </button>
          <button type="button" className="btn btn-ghost" onClick={() => setOpen(!open)}>
            {open ? "Hide" : pem.trim() ? "Show" : "Paste"}
          </button>
          {pem.trim() && (
            <button type="button" className="btn btn-ghost" onClick={() => onChange("")}>
              Remove
            </button>
          )}
        </div>
        <input
          ref={file}
          type="file"
          hidden
          accept=".pem,.crt,.cer,.der"
          onChange={(e) => {
            load(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
      </div>
      <p className={`provider-cert-state${pem.trim() ? " set" : ""}`}>
        {pem.trim()
          ? count
            ? `${count} certificate${count === 1 ? "" : "s"} — trusted instead of the public roots`
            : "No certificate found in this text"
          : "None — the server is checked against the public roots"}
      </p>
      {open && (
        <textarea
          className="provider-cert"
          rows={6}
          spellCheck={false}
          value={pem}
          placeholder={"-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----"}
          onChange={(e) => onChange(e.target.value)}
        />
      )}
      {readError && <p className="modal-note provider-error">{readError}</p>}
      <p className="modal-note">
        The server's own certificate, if it is self-signed, or the CA that issued it. A chain can be pasted whole.
      </p>
    </section>
  );
}
