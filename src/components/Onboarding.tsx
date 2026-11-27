import { useState } from "react";
import { Check, Search } from "lucide-react";
import { KiboBrand } from "./KiboBrand";
import { Field } from "./EntryField";
import { BUILTIN_PROVIDERS, KNOWN_PROVIDERS, type BuiltinProvider } from "../lib/builtinProviders";
import { filterBy } from "../lib/providerForm";
import { ONBOARDING_STEPS } from "../lib/onboarding";
import { matches } from "../lib/shortcuts";
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

type Props = {
  provider: ProviderOps;
  /** Not now: remembered, so the next launch opens the app as it is. */
  onSkip: () => void;
  /** A provider is saved and the screen is done with. */
  onFinish: () => void;
};

/**
 * The first-run screen: what the app is, and the one thing it cannot work
 * without — a provider to answer with. A layer over the body rather than a
 * dialog: with nothing to answer with there is nothing to go back to, so the
 * only way out is Skip, and the title bar above it stays the window's own
 * handle.
 */
export function Onboarding({ provider, onSkip, onFinish }: Props) {
  const [step, setStep] = useState(0);
  const first = step === 0;

  return (
    <div className="onboarding" role="dialog" aria-modal="true" aria-label="First run">
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
      {first ? <Welcome onNext={() => setStep(1)} onSkip={onSkip} /> : <ProviderStep provider={provider} onBack={() => setStep(0)} onFinish={onFinish} />}
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
          An agent that works in your folder: it reads the code, makes the change, runs the command — and shows you
          what it did.
        </p>
        <ul className="onboarding-points">
          <li>It works in a folder. Every file it touches is one of yours, and every change is a diff you can read.</li>
          <li>You see the work: the calls it makes, the files it reads, the branch it leaves behind.</li>
          <li>Your key stays on this machine. Messages go to the provider you pick, and nowhere else.</li>
        </ul>
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

/** The form as it stands; two fields and the key, the rest is Settings' business. */
type Draft = { id: string; kind: ProviderKind; baseUrl: string; entry: BuiltinProvider | null };

const BLANK: Draft = { id: "", kind: "openAiCompatible", baseUrl: "", entry: null };

/** The "Other…" card: an endpoint the catalog does not list, typed by hand. */
const OTHER: BuiltinProvider = { id: "", name: "Other", kind: "openAiCompatible", baseUrl: "" };

const hostOf = (url: string) =>
  url.replace(/^https?:\/\//, "").replace(/\/.*$/, "");

/**
 * The provider step: which provider, its key, and proof that the two work
 * before anything is saved. "Continue" asks the provider what it serves; the
 * list that comes back is what "Connect" then pins, so a key that does not
 * work is found out here rather than on the first message.
 */
function ProviderStep({ provider, onBack, onFinish }: { provider: ProviderOps; onBack: () => void; onFinish: () => void }) {
  const [draft, setDraft] = useState<Draft>(BLANK);
  const [key, setKey] = useState("");
  const [browsing, setBrowsing] = useState(false);
  // The check: its list once it passes, its refusal once it does not. What
  // comes back decides what the one primary button does.
  const [models, setModels] = useState<string[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  // What the saved provider pins; "" is the provider's default (its first).
  const [model, setModel] = useState("");
  const [saving, setSaving] = useState(false);

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

  // No `freeName`: this screen is only up when no provider is saved, so there
  // is no name to be taken by.
  const pick = (entry: BuiltinProvider) => {
    setDraft({ id: entry.id, kind: entry.kind, baseUrl: entry.baseUrl, entry });
    setBrowsing(false);
    setModels(null);
    setRefused(null);
    setModel("");
    // A key typed for one provider is not the next one's: the check would send
    // it to another endpoint, and it could be saved under another name.
    setKey("");
  };

  const edit = (next: Partial<Draft>) => {
    setDraft((prev) => ({ ...prev, ...next }));
    // The address or the name changed: what was checked no longer is.
    setModels(null);
    setRefused(null);
  };

  const check = async () => {
    if (!named || checking) return;
    setChecking(true);
    setRefused(null);
    try {
      const list = await provider.onProbe(config(null), key.trim());
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
    const stored = await provider.onSave(config(pin), key.trim(), null);
    setSaving(false);
    if (stored !== false) onFinish();
  };

  const primary = models ? "Connect" : refused ? "Try again" : "Continue";

  /** What the one primary button does, and what Enter in a field does with it. */
  const submit = () => void (models ? connect(model.trim() || null) : check());

  return (
    <div className="onboarding-page">
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
        <h1 className="onboarding-title">Connect a model</h1>
        <p className="onboarding-lead">
          Kibo answers through a provider you already have an account with. The key is sealed on this machine.
        </p>

        <div className="onboarding-providers">
          {BUILTIN_PROVIDERS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={`onboarding-provider${draft.entry?.id === entry.id ? " active" : ""}`}
              onClick={() => pick(entry)}
            >
              <span className="onboarding-provider-name">{entry.name}</span>
              <span className="onboarding-provider-host">{hostOf(entry.baseUrl)}</span>
            </button>
          ))}
          <button
            type="button"
            className={`onboarding-provider${browsing || draft.entry === OTHER ? " active" : ""}`}
            onClick={() => {
              // Its own endpoint and the catalog, both: a row fills the form,
              // and with no row the form is typed.
              if (draft.entry === OTHER) setBrowsing((open) => !open);
              else {
                pick(OTHER);
                setBrowsing(true);
              }
            }}
          >
            <span className="onboarding-provider-name">Other…</span>
            <span className="onboarding-provider-host">any compatible endpoint</span>
          </button>
        </div>

        {browsing && <Catalog onPick={pick} />}

        {draft.entry && (
          <div className="onboarding-form">
            <Field label="Name">
              <input className="entry-input" type="text" value={draft.id} onChange={(e) => edit({ id: e.target.value })} />
            </Field>
            <Field label="Base URL">
              <input className="entry-input" type="text" value={draft.baseUrl} onChange={(e) => edit({ baseUrl: e.target.value })} />
            </Field>
            <Field
              label="API key"
              hint={draft.id === "ollama" ? "Nothing to paste — Ollama runs on this machine." : "Sealed on this machine, and never read back."}
            >
              <input
                className="entry-input"
                type="password"
                value={key}
                placeholder={draft.id === "ollama" ? "not needed" : "paste the key"}
                onChange={(e) => {
                  setKey(e.target.value);
                  // A different key is a different check.
                  setModels(null);
                  setRefused(null);
                }}
              />
            </Field>
            {draft.entry.trustedCertPem && <p className="onboarding-note">Its own certificate travels with it.</p>}
          </div>
        )}

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
            <p className="onboarding-note">
              The provider refused the address and the key above. Both stay editable — the key can also be saved
              anyway and checked later in Settings → Models.
            </p>
          </div>
        )}

        <p className="onboarding-note">
          Protocol, certificate, sampling and the context window live in Settings → Models, once this is done.
        </p>
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
