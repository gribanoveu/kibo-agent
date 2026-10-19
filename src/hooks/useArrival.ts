import { useEffect, useRef } from "react";

/**
 * Runs `apply` for each new `value` handed to a message box from outside — a
 * rewound message, a quote, a page's error — but not for the one already there
 * when the box mounts. That one was applied by the box before it; the value
 * outlives the component (it is the hook's state, or App's), and applied again
 * it would put text back the user has since sent or cleared.
 */
export function useArrival<T>(value: T | null, apply: (value: T) => void) {
  const atMount = useRef(value);
  useEffect(() => {
    if (value === null || value === atMount.current) return;
    apply(value);
  }, [value]);
}
