import { useCallback, useEffect, useRef, useState } from "react";
import { shouldOnboard } from "../lib/onboarding";
import type { LlmSettings } from "../lib/settings";
import { isBoolean, useStoredState } from "./useStoredState";

/** The "Skip" flag: this window was asked about setup and said not now. */
const DISMISSED = "atlas-onboarding-dismissed";

/**
 * The first-run screen: whether it is up, and the two ways out of it.
 *
 * The question is answered once per launch, when the settings are first read,
 * and then left alone: removing the last provider in Settings must not throw
 * the screen over the dialog that was just closed. A launch that finds a
 * provider never asks again, even if it is removed.
 */
export function useOnboarding(settings: LlmSettings | null) {
  const [dismissed, setDismissed] = useStoredState(DISMISSED, false, isBoolean);
  const [open, setOpen] = useState(false);
  // Whether the question has been answered for this launch. The settings
  // arrive after the first render, so there is nothing to answer yet.
  const decided = useRef(false);

  useEffect(() => {
    if (decided.current || settings === null) return;
    decided.current = true;
    setOpen(shouldOnboard(settings, dismissed));
  }, [settings, dismissed]);

  /** Skipped: remembered, so the next launch opens the app as it is. */
  const dismiss = useCallback(() => {
    setDismissed(true);
    setOpen(false);
  }, [setDismissed]);

  /** Set up: closed, and not remembered as skipped — the provider saved is the record. */
  const finish = useCallback(() => setOpen(false), []);

  return { open, dismiss, finish };
}
