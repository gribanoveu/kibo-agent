import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Info } from "lucide-react";
import { matches } from "../lib/shortcuts";
import "./InfoTip.css";

/**
 * An info icon with a tooltip of its own: hover or focus shows it, a click
 * pins it open, Escape or a click outside closes it — the app draws its own,
 * as Dropdown and Modal do, not the platform's `title`. Reusable wherever a
 * label needs a sentence of explanation.
 */
export function InfoTip({ text, below = false }: { text: string; below?: boolean }) {
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

  // Centred on the icon, unless that runs past the edge of what clips it — a
  // sheet's scrolling body, the window — then moved back inside by the overrun.
  const pop = useRef<HTMLSpanElement>(null);
  const [shift, setShift] = useState(0);
  useLayoutEffect(() => {
    const el = pop.current;
    if (!shown || !el) return setShift(0);
    const clips = (node: Element) => {
      const { overflow, overflowX, overflowY } = getComputedStyle(node);
      return /auto|scroll|hidden|clip/.test(`${overflow} ${overflowX} ${overflowY}`);
    };
    let clip = el.parentElement;
    while (clip && !clips(clip)) clip = clip.parentElement;
    const edge = clip?.getBoundingClientRect();
    const left = Math.max(edge?.left ?? 0, 0) + 8;
    const right = Math.min(edge?.right ?? window.innerWidth, window.innerWidth) - 8;
    const box = el.getBoundingClientRect();
    setShift(box.left < left ? left - box.left : box.right > right ? right - box.right : 0);
  }, [shown]);

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
        <span ref={pop} role="tooltip" className={`info-tip-pop${below ? " below" : ""}`} style={shift ? { transform: `translateX(calc(-50% + ${shift}px))` } : undefined}>
          {text}
        </span>
      )}
    </span>
  );
}