import { useState } from "react";
import { Check, Search } from "lucide-react";
import { KiboBrand } from "./KiboBrand";
import { ProviderLogo } from "./ProviderLogo";
import { useHeldShortcuts } from "../hooks/useShortcuts";
import { Field } from "./EntryField";
import { BUILTIN_PROVIDERS, KNOWN_PROVIDERS, type BuiltinProvider } from "../lib/builtinProviders";
import { filterBy } from "../lib/providerForm";
import { ONBOARDING_STEPS, extrasFor, isDeepSeek } from "../lib/onboarding";
import { matches } from "../lib/shortcuts";
import type { FeatureView } from "../lib/features";
import type { ProviderConfig, ProviderKind } from "../lib/settings";
import "./Onboarding.css";

/** Saving a provider and asking one for its models — what Settings' form is given too. */
export type ProviderOps = {
  busy: boolean;
  error: string | null;
  /** Resolves to whether it was stored, as the settings form's save does. */
  onSave: (provider: ProviderConfig, apiKey: string | null, renamedFrom: string | null) => void | Promise<boolean>;
  /** What `provider` serves, asked before it is saved — the same live call the form's "Load list" makes. */
  onProbe: (provider: ProviderConfig, apiKey: string | null) => Promise<string[]>;
};

/** The features the app leaves off until asked for, and a switch for each. */
export type FeatureOps = {
  /** What the build says it should be until the user chooses: the list carries both. */
  list: FeatureView[];
  /** Turns one on or off; the app reads the list back itself. */
  onSet: (id: string, on: boolean) => void | Promise<void>;
  error?: string | null;
};

type Props = {
  provider: ProviderOps;
  features: FeatureOps;
  /** Not now: remembered, so the next launch opens the app as it is. */
  onSkip: () => void;
  /** A provider is saved and the screen is done with. */
  onFinish: () => void;
  /** The screen is done, and the window itself is shown instead — a step at a time. */
  onTour: () => void;
};

/**
 * The first-run screen: what the app is, and the one thing it cannot work
 * without — a provider to answer with. A layer over the body rather than a
 * dialog: with nothing to answer with there is nothing to go back to, so the
 * only way out is Skip, and the title bar above it stays the window's own
 * handle.
 *
 * Choosing and connecting are two pages: the cards are a choice, and the key,
 * the address and the model list want the whole window, not the room left
 * under a grid of them.
 */
export function Onboarding({ provider, features, onSkip, onFinish, onTour }: Props) {
  const [step, setStep] = useState(0);
  // The window's keys are the panes' business, and those panes are still
  // mounted behind this layer: held, so ⌘/ cannot open a dialog nobody sees
  // and ⌘E cannot close a file that is not on screen.
  useHeldShortcuts();
  // What is being connected, and the key it takes — here rather than in a step,
  // so going back to the cards and returning finds both as they were: a blank
  // key would be saved as "no key", and that deletes the one that was there.
  const [draft, setDraft] = useState<Draft>(BLANK);
  const [key, setKey] = useState("");

  const choose = (entry: BuiltinProvider) => {
    // A key typed for one provider is not the next one's: the check would send
    // it to another endpoint, and it could be saved under another name.
    if (draft.entry?.id !== entry.id) setKey("");
    setDraft({ id: entry.id, kind: entry.kind, baseUrl: entry.baseUrl, entry });
    setStep(2);
  };

  const edit = (next: Partial<Draft>) => setDraft((prev) => ({ ...prev, ...next }));

  return (
    <div className="onboarding text-scale" role="dialog" aria-modal="true" aria-label="First run">
      <div className="onboarding-top">
        <div className="onboarding-dots" aria-hidden="true">
          {ONBOARDING_STEPS.map((name, i) => (
            <span key={name} className={`onboarding-dot${i === step ? " active" : ""}`} />
          ))}
        </div>
        <button className="onboarding-skip" type="button" onClick={onSkip}>
          Skip
        </button>
      </div>
      {step === 0 && <Welcome onNext={() => setStep(1)} onSkip={onSkip} />}
      {/* Kept mounted while a later step is up, so the way back finds the page
          as it was left rather than a fresh one. */}
      {step >= 1 && <ProviderStep hidden={step !== 1} onBack={() => setStep(0)} onChoose={choose} />}
      {step >= 2 && (
        <ConnectStep
          provider={provider}
          draft={draft}
          apiKey={key}
          hidden={step !== 2}
          onEdit={edit}
          onKey={setKey}
          onBack={() => setStep(1)}
          onDone={() => setStep(3)}
        />
      )}
      {step === 3 && (
        <ExtrasStep
          features={features}
          // A balance and a search are DeepSeek's own endpoints: with another
          // provider the two switches would change nothing, so they are not
          // offered here — the Catalog still has them.
          deepseek={isDeepSeek(draft.baseUrl)}
          onBack={() => setStep(2)}
          onNext={() => setStep(4)}
        />
      )}
      {step === 4 && (
        <TourStep
          onTour={() => {
            onTour();
            onFinish();
          }}
          onBack={() => setStep(3)}
          onFinish={onFinish}
        />
      )}
    </div>
  );
}

function Welcome({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <div className="onboarding-page">
      <div className="onboarding-body">
        <KiboBrand />
        <h1 className="onboarding-title">Welcome to Kibo Agent</h1>
        <p className="onboarding-lead">
          This short setup gets Kibo ready: connect a provider for the agent to answer with, and you're in.
        </p>
      </div>
      <footer className="onboarding-foot">
        <button className="btn btn-primary onboarding-btn" type="button" autoFocus onClick={onNext}>
          Get started
        </button>
        <button className="btn btn-ghost onboarding-btn" type="button" onClick={onSkip}>
          Skip for now
        </button>
      </footer>
    </div>
  );
}

/**
 * The last step: the parts of the app that stay off until asked for —
 * dictation, a MongoDB page, notes from a call. A shortcut, not a gate: every
 * one of them is a switch in the Catalog afterwards, and the agent works
 * without any of them.
 */
function ExtrasStep({
  features,
  deepseek,
  onBack,
  onNext,
}: {
  features: FeatureOps;
  /** Whether the provider just connected is DeepSeek itself. */
  deepseek: boolean;
  onBack: () => void;
  onNext: () => void;
}) {
  const extras = extrasFor(features.list, deepseek);

  return (
    <div className="onboarding-page">
      <div className="onboarding-body">
        <h1 className="onboarding-title">A few extras</h1>
        <p className="onboarding-lead">
          Some parts of Kibo stay off until you ask for them — dictation, a MongoDB page, notes from a call. Turn on
          what you'll use.
        </p>

        <div className="onboarding-extras">
          {extras.map((one) => (
            <button
              key={one.id}
              type="button"
              aria-pressed={one.on}
              className={`onboarding-extra${one.on ? " on" : ""}`}
              onClick={() => void features.onSet(one.id, !one.on)}
            >
              <span className="onboarding-extra-text">
                <span className="onboarding-extra-name">{one.name}</span>
                <span className="onboarding-extra-summary">{one.summary}</span>
              </span>
              {/* Always there, so the text does not move when it appears. */}
              <Check size={13} aria-hidden className={`onboarding-extra-check${one.on ? " on" : ""}`} />
            </button>
          ))}
        </div>
        {features.error && <p className="onboarding-error">{features.error}</p>}
        <p className="onboarding-note">All of them, and the rest, are in the Catalog whenever you want them.</p>
      </div>

      <footer className="onboarding-foot">
        <button className="btn btn-ghost onboarding-btn" type="button" onClick={onBack}>
          Back
        </button>
        <div className="onboarding-foot-end">
          <button className="btn btn-primary onboarding-btn" type="button" onClick={onNext}>
            Next
          </button>
        </div>
      </footer>
    </div>
  );
}

/**
 * The last thing asked: whether to be shown around. The screen is a setup,
 * and the window it sets up has not been seen yet — so the offer is here,
 * where the provider and the extras are already settled, rather than a tour
 * nobody asked for.
 */
function TourStep({ onTour, onBack, onFinish }: { onTour: () => void; onBack: () => void; onFinish: () => void }) {
  return (
    <div className="onboarding-page">
      <div className="onboarding-body">
        <h1 className="onboarding-title">One more thing</h1>
        <p className="onboarding-lead">
          Want a look around? Three presses through the window: where your chats are, which panels open, and the
          message box.
        </p>
      </div>

      <footer className="onboarding-foot">
        <button className="btn btn-ghost onboarding-btn" type="button" onClick={onBack}>
          Back
        </button>
        <div className="onboarding-foot-end">
          <button className="btn btn-ghost onboarding-btn" type="button" onClick={onFinish}>
            Start working
          </button>
          <button className="btn btn-primary onboarding-btn" type="button" autoFocus onClick={onTour}>
            Show me around
          </button>
        </div>
      </footer>
    </div>
  );
}

/** What is being connected, as the form has it so far. */
type Draft = { id: string; kind: ProviderKind; baseUrl: string; entry: BuiltinProvider | null };

const BLANK: Draft = { id: "", kind: "openAiCompatible", baseUrl: "", entry: null };

/** The "Other…" card: an endpoint the catalog does not list, typed by hand. */
const OTHER: BuiltinProvider = { id: "", name: "Other", kind: "openAiCompatible", baseUrl: "" };

/** The two wires the app speaks: a card brings its own, a typed endpoint picks one. */
const KINDS: [ProviderKind, string][] = [
  ["openAiCompatible", "OpenAI-compatible"],
  ["anthropic", "Anthropic"],
];

const hostOf = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/.*$/, "");

/**
 * The choice: the providers the app carries, the catalog behind them, and the
 * card for an endpoint of the user's own. Picking any of them is the next
 * page's business.
 */
function ProviderStep({
  hidden,
  onBack,
  onChoose,
}: {
  /** On screen or kept for the way back from a later step. */
  hidden: boolean;
  onBack: () => void;
  onChoose: (entry: BuiltinProvider) => void;
}) {
  const [browsing, setBrowsing] = useState(false);

  return (
    <div className="onboarding-page" hidden={hidden}>
      <div className="onboarding-body">
        <h1 className="onboarding-title">Connect a model</h1>
        <p className="onboarding-lead">
          Kibo answers through a provider you already have an account with. Pick the one you use.
        </p>

        <div className="onboarding-providers">
          {BUILTIN_PROVIDERS.map((entry) => (
            <button key={entry.id} type="button" className="onboarding-provider" onClick={() => onChoose(entry)}>
              <span className="onboarding-provider-name">{entry.name}</span>
              <span className="onboarding-provider-host">{hostOf(entry.baseUrl)}</span>
            </button>
          ))}
          <button type="button" className="onboarding-provider" onClick={() => onChoose(OTHER)}>
            <span className="onboarding-provider-name">Other…</span>
            <span className="onboarding-provider-host">any compatible endpoint</span>
          </button>
        </div>

        <button className="onboarding-more" type="button" onClick={() => setBrowsing((open) => !open)}>
          {browsing ? "Hide the list" : `Search ${KNOWN_PROVIDERS.length} more providers`}
        </button>
        {browsing && <Catalog onPick={onChoose} />}
      </div>

      <footer className="onboarding-foot">
        <button className="btn btn-ghost onboarding-btn" type="button" onClick={onBack}>
          Back
        </button>
      </footer>
    </div>
  );
}

/**
 * The connection: what the provider is called, where it answers, and the key.
 * "Continue" asks the provider what it serves; the list that comes back is
 * what "Connect" then pins, so a key that does not work is found out here
 * rather than on the first message.
 */
function ConnectStep({
  provider,
  draft,
  apiKey,
  hidden,
  onEdit,
  onKey,
  onBack,
  onDone,
}: {
  provider: ProviderOps;
  draft: Draft;
  apiKey: string;
  /** On screen or kept for the way back from the last step. */
  hidden: boolean;
  onEdit: (next: Partial<Draft>) => void;
  onKey: (key: string) => void;
  onBack: () => void;
  onDone: () => void;
}) {
  // The check: its list once it passes, its refusal once it does not. What
  // comes back decides what the one primary button does.
  const [models, setModels] = useState<string[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  // What the saved provider pins; "" is the provider's default (its first).
  const [model, setModel] = useState("");
  const [saving, setSaving] = useState(false);

  const custom = draft.entry === OTHER;
  const busy = provider.busy || saving;
  const named = !!draft.id.trim() && !!draft.baseUrl.trim();

  // `pin` null leaves the model to the provider — "auto", then its first one.
  const config = (pin: string | null = model.trim() || null): ProviderConfig => ({
    id: draft.id.trim(),
    kind: draft.kind,
    baseUrl: draft.baseUrl.trim(),
    model: pin,
    // Carried with the card: they belong to the entry that brought them, and
    // AlfaGen's gateway answers to nothing without its certificate and header.
    trustedCertPem: draft.entry?.trustedCertPem ?? null,
    requestHeaders: draft.entry?.requestHeaders ?? {},
  });

  const edit = (next: Partial<Draft>) => {
    onEdit(next);
    // The address or the name changed: what was checked no longer is.
    setModels(null);
    setRefused(null);
  };

  const check = async () => {
    if (!named || checking) return;
    setChecking(true);
    setRefused(null);
    try {
      const list = await provider.onProbe(config(null), apiKey.trim());
      setModels(list);
      setModel(list[0] ?? "");
    } catch (e) {
      setRefused(String(e));
    } finally {
      setChecking(false);
    }
  };

  const connect = async (pin: string | null) => {
    if (!named || busy) return;
    setSaving(true);
    const stored = await provider.onSave(config(pin), apiKey.trim(), null);
    setSaving(false);
    if (stored !== false) onDone();
  };

  const primary = models ? "Connect" : refused ? "Try again" : "Continue";

  /** What the one primary button does, and what Enter in a field does with it. */
  const submit = () => void (models ? connect(model.trim() || null) : check());

  return (
    <div className="onboarding-page" hidden={hidden}>
      <div
        className="onboarding-body"
        // Enter in any of the fields is the step's own button, as it is in the
        // app's other forms: a box that quietly does nothing looks exactly
        // like a box that saved. A button hears Enter as its own click.
        onKeyDown={(e) => {
          if (!matches(e, "accept") || !(e.target instanceof HTMLInputElement)) return;
          e.preventDefault();
          submit();
        }}
      >
        <ProviderLogo id={custom ? undefined : draft.entry?.id} name={draft.entry?.name} className="onboarding-logo" />
        {/* A chosen provider is named by its mark and its address below: a
            heading saying the same thing twice is a line of the window spent
            on nothing. An endpoint of its own has no mark to be named by. */}
        {custom && <h1 className="onboarding-title">Your own endpoint</h1>}
        <p className="onboarding-lead">
          The key is sealed on this machine, and messages go to the provider you pick — nowhere else.
        </p>

        <div className="onboarding-chosen">
          <span className="onboarding-chosen-host">{hostOf(draft.baseUrl) || "its own address"}</span>
          <button className="btn btn-ghost onboarding-btn" type="button" onClick={onBack}>
            Change
          </button>
        </div>

        <div className="onboarding-form">
          {custom && (
            <div className="onboarding-field">
              <span className="entry-label">Protocol</span>
              <div className="onboarding-segmented" role="radiogroup" aria-label="Protocol">
                {KINDS.map(([kind, label]) => (
                  <button
                    key={kind}
                    type="button"
                    role="radio"
                    aria-checked={draft.kind === kind}
                    className={`onboarding-segment${draft.kind === kind ? " active" : ""}`}
                    onClick={() => edit({ kind })}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>
          )}
          <Field label="Name">
            <input className="entry-input" type="text" value={draft.id} onChange={(e) => edit({ id: e.target.value })} />
          </Field>
          <Field label="Base URL">
            <input
              className="entry-input"
              type="text"
              value={draft.baseUrl}
              placeholder={draft.kind === "anthropic" ? "https://api.anthropic.com/v1" : "https://api.openai.com/v1"}
              onChange={(e) => edit({ baseUrl: e.target.value })}
            />
          </Field>
          <Field
            label="API key"
            hint={
              draft.id === "ollama"
                ? "Nothing to paste — Ollama runs on this machine."
                : "Sealed on this machine, and never read back."
            }
          >
            <input
              className="entry-input"
              type="password"
              value={apiKey}
              placeholder={draft.id === "ollama" ? "not needed" : "paste the key"}
              onChange={(e) => {
                onKey(e.target.value);
                // A different key is a different check.
                setModels(null);
                setRefused(null);
              }}
            />
          </Field>
          {draft.entry?.trustedCertPem && <p className="onboarding-note">Its own certificate travels with it.</p>}
        </div>

        {models && (
          <div className="onboarding-models">
            <div className="onboarding-models-head">
              <span>Model</span>
              <span className="onboarding-count">
                {models.length} {models.length === 1 ? "model" : "models"}
              </span>
            </div>
            <div className="onboarding-model-list" role="radiogroup" aria-label="Model">
              {(models.length ? models : [""]).map((name) => (
                <button
                  key={name || "default"}
                  type="button"
                  role="radio"
                  aria-checked={model === name}
                  className={`onboarding-model${model === name ? " active" : ""}`}
                  onClick={() => setModel(name)}
                >
                  <span>{name || "The provider's default"}</span>
                  {model === name && <Check size={12} aria-hidden />}
                </button>
              ))}
            </div>
          </div>
        )}

        {refused && (
          <div className="onboarding-refusal" role="alert">
            <p>{refused}</p>
            <p className="onboarding-note">The provider refused the address and the key above. Both stay editable.</p>
          </div>
        )}
      </div>

      <footer className="onboarding-foot">
        <button className="btn btn-ghost onboarding-btn" type="button" onClick={onBack}>
          Back
        </button>
        <div className="onboarding-foot-end">
          {provider.error && <span className="onboarding-error">{provider.error}</span>}
          {refused && (
            <button className="btn btn-ghost onboarding-btn" type="button" disabled={busy} onClick={() => void connect(null)}>
              Save anyway
            </button>
          )}
          <button
            className="btn btn-primary onboarding-btn"
            type="button"
            disabled={!named || busy || checking}
            title={named ? undefined : "Needs a name and a base URL"}
            onClick={submit}
          >
            {checking ? "Checking…" : busy ? "Saving…" : primary}
          </button>
        </div>
      </footer>
    </div>
  );
}

/** Everything else the form knows by name, searched — the same list Settings offers. */
function Catalog({ onPick }: { onPick: (entry: BuiltinProvider) => void }) {
  const [query, setQuery] = useState("");
  const shown = filterBy(KNOWN_PROVIDERS, query, (entry) => `${entry.name} ${entry.id}`);

  return (
    <div className="onboarding-catalog">
      <label className="onboarding-search">
        <Search size={12} aria-hidden />
        <input
          type="search"
          className="entry-input"
          value={query}
          placeholder={`Search ${KNOWN_PROVIDERS.length} providers`}
          onChange={(e) => setQuery(e.target.value)}
          // Enter takes the first match rather than going on to the check.
          onKeyDown={(e) => {
            if (!matches(e, "accept")) return;
            e.preventDefault();
            e.stopPropagation();
            if (shown[0]) onPick(shown[0]);
          }}
        />
      </label>
      <div className="onboarding-catalog-list">
        {shown.map((entry) => (
          <button key={entry.id} type="button" className="onboarding-catalog-row" onClick={() => onPick(entry)}>
            <span>{entry.name}</span>
            <span className="onboarding-catalog-host">{hostOf(entry.baseUrl)}</span>
          </button>
        ))}
        {!shown.length && <p className="onboarding-note">No provider matches.</p>}
      </div>
    </div>
  );
}
