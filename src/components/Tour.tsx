import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { placeCard, placeCentred, TOUR_STEPS, type Box } from "../lib/tour";
import { matches } from "../lib/shortcuts";
import { useHeldShortcuts } from "../hooks/useShortcuts";
import "./Tour.css";

/** How far the ring stands off what it points at. */
const PAD = 6;

/**
 * A few steps over the window as it is: what the step names is ringed and the
 * rest is dimmed, and the card says what it is for. The window is held while
 * the tour is up — a press or a key behind the dim would turn something on
 * that the user cannot see happening — and Escape or the last step ends it.
 * Nothing is stored: it is offered once, in the first run, and never again.
 */
export function Tour({
  onDone,
  onOpen,
}: {
  onDone: () => void;
  /**
   * Something a step needs on screen, by the id it names — or `null` for a
   * step that points at what is already there.
   */
  onOpen?: (id: string | null) => void;
}) {
  const [index, setIndex] = useState(0);
  const [ring, setRing] = useState<Box | null>(null);
  const [spot, setSpot] = useState<{ top: number; left: number } | null>(null);
  const card = useRef<HTMLDivElement>(null);
  const step = TOUR_STEPS[index];
  const last = index === TOUR_STEPS.length - 1;

  // The window's own keys are held with it: ⌘1 must not open a panel under the
  // dim, nor ⌘⇧D turn the IDE layout on behind it.
  useHeldShortcuts();

  // What the step needs opened, asked once per step — a ref, since the caller's
  // handler is a new function on every render.
  const open = useRef(onOpen);
  open.current = onOpen;
  useEffect(() => {
    open.current?.(step.opens ?? null);
    // A panel the step points into is opened by a press on its own button, not
    // by a second copy of what the app keeps in it. The button is a toggle:
    // pressed only while the panel is closed, so a return to the step does not
    // press shut what it is here to show.
    const button = step.presses ? (document.querySelector(step.presses) as HTMLElement | null) : null;
    const opened = button && button.getAttribute("aria-expanded") !== "true";
    if (opened) button.click();
    return () => {
      // What the step opened is put away on the way out — the next step, Done
      // or Escape alike — so the tour never leaves a panel over the window.
      if (opened && button && button.getAttribute("aria-expanded") === "true") button.click();
    };
  }, [step.opens, step.presses]);

  // Escape leaves — read here rather than through the shortcuts of the window,
  // which are held: this one is the way out of the tour.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!matches(e, "close")) return;
      e.preventDefault();
      onDone();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onDone]);

  // Measured after every render of a step, and again when the window moves:
  // what is pointed at may be a panel the user opened or closed.
  useLayoutEffect(() => {
    let waited = 0;
    let retry: ReturnType<typeof setTimeout> | null = null;
    const measure = () => {
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      const bounds = card.current?.getBoundingClientRect();
      const size = { width: bounds?.width || 340, height: bounds?.height || 170 };

      const found = document.querySelector(step.target)?.getBoundingClientRect();
      if (!found || (found.width === 0 && found.height === 0)) {
        // Nothing to point at — a layout without it — so the card speaks from
        // the middle rather than vanishing; a step that opens its own subject
        // gets a moment for it to arrive, and a few more in case it is slow.
        setRing(null);
        setSpot(placeCentred(size, viewport));
        if (!found && waited < 10) {
          waited += 1;
          retry = setTimeout(measure, 120);
        }
        return;
      }
      setRing({ top: found.top - PAD, left: found.left - PAD, width: found.width + PAD * 2, height: found.height + PAD * 2 });
      setSpot(placeCard({ top: found.top, left: found.left, width: found.width, height: found.height }, size, viewport));
    };
    measure();
    window.addEventListener("resize", measure);
    // Captured, so a pane scrolled inside the window moves the ring too.
    window.addEventListener("scroll", measure, true);
    // A panel opening or closing animates its width — the sidebar does, and
    // the tour opens it — so the ring follows the box as it grows rather than
    // staying where it stood before the animation.
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    const target = document.querySelector(step.target);
    if (target) observer?.observe(target);
    return () => {
      if (retry) clearTimeout(retry);
      observer?.disconnect();
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
    };
  }, [index, step.target]);

  return (
    <div className="tour">
      {/* Everything the window would take a press on is covered: a button
          pressed behind the dim looks like nothing happened until the tour is
          gone and the panel or the layout is already open. */}
      <div className={ring ? "tour-catch" : "tour-catch tour-catch-dim"} />
      {ring && (
        // The hole is the ring's own shadow turned inside out: everything but
        // the thing pointed at is dimmed.
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
