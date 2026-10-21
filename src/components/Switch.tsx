import "./Switch.css";

/** An on/off switch, drawn rather than the platform's checkbox: a track and a
    knob that slides across. `label` names it for whoever cannot see it; text
    beside it goes in a `<label>` around it, which a click on toggles too. */
export function Switch({ label, on, onChange }: { label: string; on: boolean; onChange: (on: boolean) => void }) {
  return <button type="button" role="switch" aria-checked={on} aria-label={label} className={`switch${on ? " on" : ""}`} onClick={() => onChange(!on)} />;
}
