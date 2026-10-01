import { useCallback, useEffect, useState } from "react";
import {
  listModels,
  llmSettings,
  probeModels,
  removeProvider,
  saveApiKey,
  saveProvider,
  setActiveProvider,
  setDebugLogging,
  setKeyStore,
  setReplyLanguage,
  setTurnLimits,
  type KeyStore,
  type TurnLimits,
  type ReplyLanguage,
  type LlmSettings,
  type ProviderConfig,
} from "../lib/chat";

/** Which provider a turn talks to, and what it needs to get there. */
export function useLlmSettings() {
  const [settings, setSettings] = useState<LlmSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      setSettings(await llmSettings());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  const guard = useCallback(
    async (work: () => Promise<void>) => {
      setBusy(true);
      setError(null);
      try {
        await work();
        await reload();
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [reload],
  );

  const save = useCallback(
    (provider: ProviderConfig, apiKey: string | null) =>
      guard(async () => {
        await saveProvider(provider);
        // Only when the user typed one: an untouched field means "leave the
        // stored key alone", and sending its empty value would delete it.
        if (apiKey !== null) await saveApiKey(provider.id, apiKey);
        await setActiveProvider(provider.id);
      }),
    [guard],
  );

  const remove = useCallback((id: string) => guard(() => removeProvider(id)), [guard]);
  const select = useCallback((id: string) => guard(() => setActiveProvider(id)), [guard]);
  const keyStore = useCallback((store: KeyStore) => guard(() => setKeyStore(store)), [guard]);
  const debugLogging = useCallback(
    (enabled: boolean) => guard(() => setDebugLogging(enabled)),
    [guard],
  );
  const replyLanguage = useCallback(
    (language: ReplyLanguage) => guard(() => setReplyLanguage(language)),
    [guard],
  );
  const turnLimits = useCallback((limits: TurnLimits) => guard(() => setTurnLimits(limits)), [guard]);

  // What each provider serves, asked when the model menu opens: a live call
  // per provider, so not on every render. A provider that does not answer
  // keeps only its pinned model.
  const [served, setServed] = useState<Record<string, string[]>>({});
  const loadModels = useCallback(() => {
    for (const provider of settings?.providers ?? []) {
      listModels(provider.id)
        .then((list) => setServed((prev) => ({ ...prev, [provider.id]: list })))
        .catch(() => {});
    }
  }, [settings]);

  /** What the form's provider serves. Kept for the composer's menu too, once it is saved. */
  const probe = useCallback(async (provider: ProviderConfig, apiKey: string | null) => {
    const list = await probeModels(provider, apiKey);
    setServed((prev) => ({ ...prev, [provider.id]: list }));
    return list;
  }, []);

  /** Makes `providerId` active and pins `model` on it. */
  const pickModel = useCallback(
    (providerId: string, model: string | null) => {
      const provider = settings?.providers.find((p) => p.id === providerId);
      if (!provider) return;
      return guard(async () => {
        if ((provider.model ?? null) !== model) await saveProvider({ ...provider, model });
        await setActiveProvider(providerId);
      });
    },
    [settings, guard],
  );

  /** Sets the active provider's thinking level; `null` sends none, leaving the model's default. */
  const pickEffort = useCallback(
    (effort: string | null) => {
      const provider = settings?.providers.find((p) => p.id === settings.activeProviderId);
      if (!provider) return;
      return guard(() => saveProvider({ ...provider, reasoningEffort: effort }));
    },
    [settings, guard],
  );

  return {
    settings,
    error,
    busy,
    save,
    remove,
    select,
    keyStore,
    debugLogging,
    replyLanguage,
    turnLimits,
    models: modelChoices(settings, served),
    loadModels,
    pickModel,
    pickEffort,
    probe,
    served,
  };
}

/** One model the composer offers: `provider/model`, `model` `null` for "auto". */
export type ModelChoice = { providerId: string; model: string | null; label: string };

/** The key of a choice in a menu — the provider and the model, which can itself contain "/". */
export const choiceKey = (choice: Pick<ModelChoice, "providerId" | "model">) =>
  `${choice.providerId}\n${choice.model ?? ""}`;

/**
 * Every model the composer can switch to, as `provider/model` in lower case,
 * with the active one first-class: `current` is what the chip shows. A
 * provider without a pinned model is "auto" — the first one it lists, pinned
 * on the first turn (`services::llm_session::effective_model`).
 */
export function modelChoices(settings: LlmSettings | null, served: Record<string, string[]>) {
  const choices: ModelChoice[] = [];
  for (const provider of settings?.providers ?? []) {
    const names = [...new Set([provider.model, ...(served[provider.id] ?? [])].filter((m): m is string => !!m))];
    const models = names.length ? names : [null];
    for (const model of models) {
      choices.push({ providerId: provider.id, model, label: `${provider.id}/${model ?? "auto"}`.toLowerCase() });
    }
  }
  const active = settings?.providers.find((p) => p.id === settings.activeProviderId);
  const current = active
    ? (choices.find((c) => c.providerId === active.id && c.model === (active.model ?? null)) ?? null)
    : null;
  return { choices, current, effort: active?.reasoningEffort ?? null };
}
