import { useLayoutEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { placeCard, placeCentred, TOUR_STEPS, type Box } from "../lib/tour";
import { useShortcuts } from "../hooks/useShortcuts";
import "./Tour.css";

/** How far the ring stands off what it points at. */
const PAD = 6;

/**
 * A few steps over the window as it is: what the step names is ringed and the
 * rest is dimmed, and the card says what it is for. The window stays usable
 * under it — the tour points, the user tries — and Escape or the last step ends
 * it. Nothing is stored: it is offered once, in the first run, and never again.
 */
export function Tour({ onDone }: { onDone: () => void }) {
  const [index, setIndex] = useState(0);
  const [ring, setRing] = useState<Box | null>(null);
  const [spot, setSpot] = useState<{ top: number; left: number } | null>(null);
  const card = useRef<HTMLDivElement>(null);
  const step = TOUR_STEPS[index];
  const last = index === TOUR_STEPS.length - 1;

  useShortcuts({ close: onDone });

  // Measured after every render of a step, and again when the window moves:
  // the thing pointed at may be a panel the user just opened or closed.
  useLayoutEffect(() => {
    const measure = () => {
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      const bounds = card.current?.getBoundingClientRect();
      const size = { width: bounds?.width || 340, height: bounds?.height || 170 };

      const found = document.querySelector(step.target)?.getBoundingClientRect();
      if (!found || (found.width === 0 && found.height === 0)) {
        // Nothing to point at — a layout without it — so the card speaks from
        // the middle rather than vanishing.
        setRing(null);
        setSpot(placeCentred(size, viewport));
        return;
      }
      setRing({ top: found.top - PAD, left: found.left - PAD, width: found.width + PAD * 2, height: found.height + PAD * 2 });
      setSpot(placeCard({ top: found.top, left: found.left, width: found.width, height: found.height }, size, viewport));
    };
    measure();
    window.addEventListener("resize", measure);
    // Captured, so a pane scrolled inside the window moves the ring too.
    window.addEventListener("scroll", measure, true);
    return () => {
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
    };
  }, [index, step.target]);

  return (
    <div className="tour">
      {ring && (
        // The hole is the ring's own shadow turned inside out: everything but
        // the thing pointed at is dimmed, and a click goes through to it.
        <div className="tour-ring" aria-hidden style={ring} />
      )}
      <div
        className="tour-card"
        role="dialog"
        aria-label={`Tour: ${step.title}`}
        // Off screen until measured, rather than drawn once in the corner.
        style={{ top: spot?.top ?? -9999, left: spot?.left ?? -9999 }}
      >
        <div className="tour-head">
          <span className="tour-count">
            {index + 1} / {TOUR_STEPS.length}
          </span>
          <button className="iconbtn" type="button" title="Leave the tour" aria-label="Leave the tour" onClick={onDone}>
            <X size={14} />
          </button>
        </div>
        <h2 className="tour-title">{step.title}</h2>
        <p className="tour-body">{step.body}</p>
        <div className="tour-foot">
          <button
            className="btn btn-ghost"
            type="button"
            disabled={index === 0}
            onClick={() => setIndex((at) => Math.max(0, at - 1))}
          >
            Back
          </button>
          <button
            className="btn btn-primary"
            type="button"
            autoFocus
            onClick={() => (last ? onDone() : setIndex((at) => at + 1))}
          >
            {last ? "Done" : "Next"}
          </button>
        </div>
      </div>
    </div>
  );
}
