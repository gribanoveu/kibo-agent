// When the first-run screen is up, and what it walks through: pure, so the
// rule it rests on is a test of its own.

import type { LlmSettings } from "./settings";

/** The steps, in the order the dots show and Back and Next move. */
export const ONBOARDING_STEPS = ["welcome", "provider", "connect", "ready"] as const;

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
