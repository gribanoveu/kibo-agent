import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "lucide-react";
import "./Dropdown.css";
import { matches } from "../lib/shortcuts";

/** `label` when what the caller sends and what the reader sees differ — a wire
    value like "pullRequest" is not a word to put in a menu. */
type Option = {
  value: string;
  label?: string;
  hint?: string;
  /** Drawn under the option before it, as something that belongs to it. */
  nested?: boolean;
  /** A button at the row's right edge, doing something to the option rather
      than picking it. Unavailable, it stays in place and its `title` says why. */
  action?: { icon: ReactNode; title: string; unavailable?: boolean; onRun: () => void };
};

type Props = {
  label: ReactNode;
  title?: string;
  options: Option[];
  value: string;
  onPick: (value: string) => void;
  emptyLabel?: string;
  /** Where the menu opens; above by default — the composer sits at the bottom. */
  below?: boolean;
  /** Anchors the menu to the trigger's right edge — for a trigger at the right of a narrow panel. */
  right?: boolean;
  /** A title over the options, saying what question they answer. */
  heading?: string;
  /** Called as the menu opens — for options that are fetched only when wanted. */
  onOpen?: () => void;
  /** A box above the options for a value not among them — a namespace the
      user may work in but not list. Enter picks what was typed.

      `onType` says the same of every keystroke, for a caller that may be
      confirmed while this menu is open: a dialog's own button is pressed
      without Enter, and text left only in the box would be lost. */
  custom?: { placeholder: string; onEnter: (value: string) => void; onType?: (value: string) => void };
  /** A line under the heading about the options themselves — why some are missing. */
  note?: string;
  /** Shown but not changeable: the menu does not open, and `title` says why. */
  locked?: boolean;
};

/** Trigger + role="listbox" menu — the app draws its own dropdowns, never <select>. */
export function Dropdown({ label, title, options, value, onPick, emptyLabel, below, right, heading, onOpen, custom, note, locked = false }: Props) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  /**
   * Where the menu sits while it is drawn; `null` before it is placed.
   *
   * The menu is portaled to the viewport: inside a scrolling panel — and a
   * dialog's body scrolls too, with a height of its own — the panel's edge cuts
   * it off, and takes it away when it scrolls. Being outside means its place in
   * the viewport is worked out from the trigger.
   */
  const [at, setAt] = useState<{ top: number; left: number; minWidth: number; maxWidth: number } | null>(null);
  /** What has been typed into the custom box: the options narrow to it. */
  const [typed, setTyped] = useState("");

  // Placed after every render while open: `below` is the side it was asked for,
  // and the other side is taken when that one has no room.
  useLayoutEffect(() => {
    if (!open) return;
    const button = wrap.current?.getBoundingClientRect();
    const box = menu.current?.getBoundingClientRect();
    if (!button || !box) return;
    const gap = 6;
    const edge = 8;
    const down = button.bottom + gap;
    const up = button.top - gap - box.height;
    const wanted = below ? down : up;
    const other = below ? up : down;
    const fits = (top: number) => top >= edge && top + box.height <= window.innerHeight - edge;
    const top = fits(wanted) ? wanted : fits(other) ? other : Math.max(edge, Math.min(wanted, window.innerHeight - edge - box.height));
    const room = right ? button.right - box.width : button.left;
    const left = Math.max(edge, Math.min(room, window.innerWidth - edge - box.width));
    // A menu no narrower than the trigger it hangs from: a field's list is the
    // field's width, and the list narrowing under the typing does not change
    // how wide it is — a box that resized as it filtered read as a jump.
    const floor = Math.min(button.width, window.innerWidth - edge * 2);
    const minWidth = Math.max(220, floor);
    const maxWidth = Math.max(360, floor);
    setAt((was) =>
      was && Math.abs(was.top - top) < 0.5 && Math.abs(was.left - left) < 0.5 && was.minWidth === minWidth && was.maxWidth === maxWidth
        ? was
        : { top, left, minWidth, maxWidth },
    );
  });

  // Closing empties the box, so the next opening is the whole list again.
  useEffect(() => {
    if (!open) setTyped("");
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!wrap.current?.contains(target) && !menu.current?.contains(target)) setOpen(false);
    };
    // In the capture phase, so the menu has this Escape before anything round
    // it, whichever listened first; what it took, `defaultPrevented` says.
    const onKey = (e: KeyboardEvent) => {
      if (!matches(e, "close")) return;
      e.preventDefault();
      setOpen(false);
    };
    // What scrolled is what the trigger was measured against: the menu has moved.
    const onScroll = (e: Event) => {
      if (e.target instanceof Node && wrap.current && e.target.contains(wrap.current)) setOpen(false);
    };
    const onResize = () => setOpen(false);
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
    };
  }, [open]);

  // Typing in the box narrows the options to the word typed: in a cluster of
  // hundreds of namespaces, the ones it names are the list.
  const said = typed.trim().toLowerCase();
  const shown = said ? options.filter((opt) => (opt.label ?? opt.value).toLowerCase().includes(said)) : options;

  return (
    <div className="dropdown-wrap" ref={wrap}>
      <button
        type="button"
        className={`chip${open ? " open" : ""}`}
        title={title}
        aria-haspopup="listbox"
        aria-expanded={open}
        // Not `disabled`: a disabled button shows no tooltip, and the tooltip is the reason.
        aria-disabled={locked || undefined}
        onClick={() => {
          if (locked) return;
          if (!open) {
            onOpen?.();
            // On screen at once and measured from here: a menu drawn at the
            // viewport's corner to be measured is a menu someone sees flicker.
            const button = wrap.current?.getBoundingClientRect();
            setAt(
              button
                ? { top: below ? button.bottom + 6 : button.top - 6, left: right ? button.right : button.left, minWidth: 220, maxWidth: 360 }
                : null,
            );
          }
          setOpen(!open);
        }}
      >
        <span className="chip-label">{label}</span>
        {!locked && <ChevronDown className="chip-chev" size={10} />}
      </button>
      {open &&
        createPortal(
          // Portaled out of the trigger's own element, but a press in the menu
          // is a press on that element's control: a panel, sheet or dialog that
          // closes itself on a press outside it would otherwise close on the
          // option — and take the option with it before the click lands, so the
          // pick was lost.
          <div
            ref={menu}
            className="dropdown-menu"
            role="listbox"
            aria-label={heading}
            style={{ position: "fixed", top: at?.top ?? 0, left: at?.left ?? 0, minWidth: at?.minWidth, maxWidth: at?.maxWidth }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            {heading && <div className="dropdown-heading">{heading}</div>}
            {custom && (
              <input
                className="dropdown-custom"
                type="text"
                autoFocus
                placeholder={custom.placeholder}
                aria-label={custom.placeholder}
                onChange={(e) => {
                  setTyped(e.currentTarget.value);
                  custom.onType?.(e.currentTarget.value);
                }}
                // A form field's Enter, as in every settings form here — not a shortcut.
                onKeyDown={(e) => {
                  const typed = e.currentTarget.value.trim();
                  if (!matches(e, "accept") || !typed) return;
                  e.preventDefault();
                  custom.onEnter(typed);
                  setOpen(false);
                }}
              />
            )}
            {note && <div className="dropdown-note">{note}</div>}
            {shown.length === 0 && (said || !custom) && (
              <div className="dropdown-empty">{said ? `Nothing matches “${typed.trim()}”` : emptyLabel ?? "Nothing here yet"}</div>
            )}
            {shown.map((opt) => {
              const item = (
                <button
                  key={opt.value}
                  type="button"
                  role="option"
                  aria-selected={opt.value === value}
                  className={`dropdown-item${opt.value === value ? " active" : ""}${opt.nested ? " nested" : ""}`}
                  onClick={() => {
                    onPick(opt.value);
                    setOpen(false);
                  }}
                >
                  <span className="dropdown-item-text">
                    <span className="dropdown-item-label">{opt.label ?? opt.value}</span>
                    {opt.hint && <span className="hint">{opt.hint}</span>}
                  </span>
                  {opt.value === value && <Check className="dropdown-check" size={13} />}
                </button>
              );
              const action = opt.action;
              if (!action) return item;
              // Beside the option, not inside it: a button in a button is not valid HTML.
              return (
                <div key={opt.value} className="dropdown-row">
                  {item}
                  <button
                    type="button"
                    className="dropdown-action"
                    title={action.title}
                    aria-label={action.title}
                    // Not `disabled`: a disabled button shows no tooltip, and the
                    // tooltip is what says why it cannot be used.
                    aria-disabled={action.unavailable || undefined}
                    onClick={() => {
                      if (action.unavailable) return;
                      action.onRun();
                      setOpen(false);
                    }}
                  >
                    {action.icon}
                  </button>
                </div>
              );
            })}
          </div>,
          document.body,
        )}
    </div>
  );
}
