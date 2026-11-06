import { useState } from "react";

/**
 * A control whose value lives on the backend, and the one rule about changing
 * it: the control moves **after** the backend has agreed.
 *
 * Both of the composer's chips are this. The backend is where each setting is
 * enforced — one decides which tools reach the model, the other whether a call
 * pauses for a human — so a chip that moved first would be showing a state the
 * agent is not in. "Analyst" over a turn that is still armed, or "Ask" over one
 * that will not ask, is worse than the old label for a moment longer.
 *
 * `pick` returns the reason on failure rather than raising or toasting: a hook
 * does not know where this app puts its errors.
 */
export function useBackendSetting<T>(apply: (value: T) => Promise<void>, initial: T) {
  const [value, setValue] = useState(initial);

  const pick = async (next: T): Promise<string | null> => {
    try {
      await apply(next);
      setValue(next);
      return null;
    } catch (e) {
      return String(e);
    }
  };

  return { value, pick };
}
