import { useEffect, useRef, useState } from "react";
import { Info } from "lucide-react";
import { matches } from "../lib/shortcuts";
import "./InfoTip.css";

/**
 * An info icon with a tooltip of its own: hover or focus shows it, a click
 * pins it open, Escape or a click outside closes it — the app draws its own,
 * as Dropdown and Modal do, not the platform's `title`. Reusable wherever a
 * label needs a sentence of explanation.
 */
export function InfoTip({ text }: { text: string }) {
  const [hovered, setHovered] = useState(false);
  const [pinned, setPinned] = useState(false);
  const wrap = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (!pinned) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) {
        setPinned(false);
        setHovered(false);
      }
    };
    // A real click focuses the button first, so focus's show is let go of
    // too — else Escape and the second click would leave it open.
    const onKey = (e: KeyboardEvent) => {
      if (matches(e, "close")) {
        setPinned(false);
        setHovered(false);
      }
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [pinned]);

  const shown = hovered || pinned;
  return (
    <span className="info-tip" ref={wrap}>
      <button
        type="button"
        className="info-tip-btn"
        aria-label="More info"
        aria-expanded={shown}
        onPointerEnter={() => setHovered(true)}
        onPointerLeave={() => setHovered(false)}
        onFocus={() => setHovered(true)}
        onBlur={() => setHovered(false)}
        onClick={() => {
          setPinned((v) => !v);
          setHovered(false);
        }}
      >
        <Info size={12} aria-hidden />
      </button>
      {shown && (
        <span role="tooltip" className="info-tip-pop">
          {text}
        </span>
      )}
    </span>
  );
}