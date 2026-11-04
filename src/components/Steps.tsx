import "./Steps.css";

/**
 * The steps of a dialog: a strip across the top, the current one marked, the
 * ones ahead not reachable until the step before them is passed. A step behind
 * can be clicked back to.
 */
export function Steps<T extends string>({
  steps,
  current,
  onGo,
}: {
  steps: { id: T; label: string }[];
  current: T;
  onGo: (id: T) => void;
}) {
  const at = steps.findIndex((step) => step.id === current);
  return (
    <nav className="steps" aria-label="Steps">
      {steps.map(({ id, label }, i) => (
        <button
          key={id}
          type="button"
          className="step"
          aria-current={current === id ? "step" : undefined}
          title={i > at ? "Fill this step in first" : undefined}
          disabled={i > at}
          onClick={() => onGo(id)}
        >
          <span className="step-num">{i + 1}</span>
          {label}
        </button>
      ))}
    </nav>
  );
}
