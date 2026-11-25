import { useCallback, useEffect, useState } from "react";
import { type KeyStore, listModels, llmSettings, type LlmSettings, probeModels, type ProviderConfig, removeProvider, type ReplyLanguage, saveApiKey, saveProvider, setActiveProvider, setDebugLogging, setKeyStore, setReplyLanguage, setTurnLimits, type TurnLimits } from "../lib/settings";
import type { ChatModel } from "../lib/savedChats";

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
    (provider: ProviderConfig, apiKey: string | null, renamedFrom: string | null = null) =>
      guard(async () => {
        // `renamedFrom` is the name the form was opened on: a provider saved
        // under another one is that provider renamed, not a second one beside
        // the one it was renamed from.
        await saveProvider(provider, renamedFrom);
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

  /** Sets a provider's thinking level — the open chat's by default, else the
   * active one; `null` sends none, leaving the model's default. The level is
   * the provider's own: two chats on one provider share it. */
  const pickEffort = useCallback(
    (effort: string | null, providerId?: string) => {
      const provider = settings?.providers.find((p) => p.id === (providerId ?? settings.activeProviderId));
      if (!provider) return;
      return guard(() => saveProvider({ ...provider, reasoningEffort: effort }));
    },
    [settings, guard],
  );

  return {
    settings,
    error,
    busy,
    reload,
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
 * with the open chat's own first-class: `current` is what the chip shows —
 * the chat's pin when it has one, else the active provider's. A provider
 * without a pinned model is "auto" — the first one it lists, pinned on the
 * first turn (`services::llm_session::effective_model`). Once the list is
 * known, the auto choice names that model, so the chip says what would go
 * out rather than "no model".
 */
export function modelChoices(settings: LlmSettings | null, served: Record<string, string[]>, chatModel: ChatModel | null = null) {
  const choices: ModelChoice[] = [];
  for (const provider of settings?.providers ?? []) {
    const listed = [...new Set(served[provider.id] ?? [])];
    const names = [...new Set([provider.model, ...listed].filter((m): m is string => !!m))];
    // Auto stays a choice while nothing is pinned, even once the list is
    // known: without it `current` would find nothing and the chip would say
    // "no model" with a provider ready to answer.
    const models = provider.model ? names : [null, ...names];
    for (const model of models) {
      const shown = model ?? (listed[0] ? `${listed[0]} (auto)` : "auto");
      choices.push({ providerId: provider.id, model, label: `${provider.id}/${shown}`.toLowerCase() });
    }
  }
  const active = settings?.providers.find((p) => p.id === settings.activeProviderId);
  // The chat's pin, while its provider stands. Its model may be one the
  // provider no longer lists — still what the turn sends, so still shown;
  // "auto" under a pinned provider is a choice of its own, naming the model
  // the provider would send.
  const pinned = chatModel ? settings?.providers.find((p) => p.id === chatModel.provider) : undefined;
  if (chatModel && pinned && !choices.some((c) => c.providerId === pinned.id && c.model === (chatModel.model ?? null))) {
    const want = chatModel.model ?? null;
    const auto = pinned.model ?? [...new Set(served[pinned.id] ?? [])][0];
    const shown = want ?? (auto ? `${auto} (auto)` : "auto");
    choices.push({ providerId: pinned.id, model: want, label: `${pinned.id}/${shown}`.toLowerCase() });
  }
  // A pin whose provider is gone shows the active one: the backend
  // substitutes it on the next read, and "no model" would lie about the turn.
  const current = pinned && chatModel
    ? (choices.find((c) => c.providerId === pinned.id && c.model === (chatModel.model ?? null)) ?? null)
    : active
      ? (choices.find((c) => c.providerId === active.id && c.model === (active.model ?? null)) ?? null)
      : null;
  return { choices, current, effort: (pinned ?? active)?.reasoningEffort ?? null };
}
