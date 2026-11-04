import type { ReactNode } from "react";
import "./EntryField.css";

/**
 * A labelled input, with a line of help under it. `menu` for a Dropdown: a
 * `<label>` around one would pass its own click on to the trigger and shut the
 * menu it just opened.
 */
export function Field({ label, hint, menu, children }: { label: string; hint?: ReactNode; menu?: boolean; children: ReactNode }) {
  const Wrap = menu ? "div" : "label";
  return (
    <Wrap className="entry-field">
      <span className="entry-label">{label}</span>
      {children}
      {hint && <span className="entry-hint">{hint}</span>}
    </Wrap>
  );
}
