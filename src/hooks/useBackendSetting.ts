import { useEffect, useRef, useState } from "react";

/**
 * A control whose value lives on the backend, and the one rule about changing
 * it: the control moves **after** the backend has agreed.
 *
 * The composer's mode is this. The backend is where it is enforced — it decides
 * which tools reach the model — so a control that moved first would be showing a
 * state the agent is not in. "Analyst" over a turn that is still armed is worse
 * than the old label for a moment longer.
 *
 * `pick` returns the reason on failure rather than raising or toasting: a hook
 * does not know where this app puts its errors.
 *
 * `read`, when the backend remembers the value, says what it is once the
 * control is up; `initial` stands until then. An answer that lands after a pick
 * was asked before it, and is dropped.
 */
export function useBackendSetting<T>(apply: (value: T) => Promise<void>, initial: T, read?: () => Promise<T>) {
  const [value, setValue] = useState(initial);
  const picked = useRef(false);

  useEffect(() => {
    read?.().then(
      (stored) => {
        if (!picked.current) setValue(stored);
      },
      () => {},
    );
  }, []);

  const pick = async (next: T): Promise<string | null> => {
    try {
      await apply(next);
      picked.current = true;
      setValue(next);
      return null;
    } catch (e) {
      return String(e);
    }
  };

  return { value, pick };
}
