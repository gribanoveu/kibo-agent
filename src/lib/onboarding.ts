// When the first-run screen is up, and what it walks through: pure, so the
// rule it rests on is a test of its own.

import type { LlmSettings } from "./settings";
import type { FeatureView } from "./features";

/** The steps, in the order the dots show and Back and Next move. */
export const ONBOARDING_STEPS = ["welcome", "provider", "connect", "extras"] as const;

export type OnboardingStep = (typeof ONBOARDING_STEPS)[number];

/**
 * Whether to show the screen: the settings have loaded, no provider is saved,
 * and it was not skipped. A provider is what makes the app able to answer at
 * all, so its absence — not a launch count — is what asks for setup.
 */
export function shouldOnboard(settings: LlmSettings | null, dismissed: boolean): boolean {
  if (dismissed || settings === null) return false;
  return settings.providers.length === 0;
}

/**
 * Whether the provider at `baseUrl` is DeepSeek itself — the host only, as
 * `domain::provider_balance::is_deepseek` reads it: the name is the user's, and
 * a gateway in front of DeepSeek has none of its own endpoints.
 */
export function isDeepSeek(baseUrl: string): boolean {
  try {
    return new URL(baseUrl.trim()).host.toLowerCase() === "api.deepseek.com";
  } catch {
    return false;
  }
}

/**
 * The features that ask DeepSeek for something — a balance, a search — so with
 * any other provider they are a switch that changes nothing. Off the wizard's
 * list for that reason alone; the Catalog still offers them.
 */
const DEEPSEEK_ONLY = ["provider-balance", "deepseek-search"];

/**
 * Never asked on a first run: Voice memory keeps a voice print of the people on
 * a call — biometric data about someone else, which is consented to by opening
 * it in the Catalog deliberately, not by a press on a welcome screen.
 */
const CATALOG_ONLY = ["voice-memory"];

/** What the last step offers: the features the build leaves off, less those it must not ask about. */
export function extrasFor(features: FeatureView[], deepseek: boolean): FeatureView[] {
  return features.filter(
    (one) =>
      !one.defaultOn &&
      !CATALOG_ONLY.includes(one.id) &&
      (deepseek || !DEEPSEEK_ONLY.includes(one.id)),
  );
}
