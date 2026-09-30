import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Check, ChevronDown } from "lucide-react";
import "./Dropdown.css";

/** `label` when what the caller sends and what the reader sees differ — a wire
    value like "plan" is not a word to put in a menu. */
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
      user may work in but not list. Enter picks what was typed. */
  custom?: { placeholder: string; onEnter: (value: string) => void };
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
  // A menu that would run past the window's right edge hangs from the trigger's right instead.
  const [flip, setFlip] = useState(false);
  useLayoutEffect(() => {
    const edge = menu.current?.getBoundingClientRect().right ?? 0;
    setFlip(open && edge > window.innerWidth);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

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
          if (!open) onOpen?.();
          setOpen(!open);
        }}
      >
        <span className="chip-label">{label}</span>
        {!locked && <ChevronDown className="chip-chev" size={10} />}
      </button>
      {open && (
        <div ref={menu} className={`dropdown-menu${below ? " below" : ""}${right || flip ? " right" : ""}`} role="listbox" aria-label={heading}>
          {heading && <div className="dropdown-heading">{heading}</div>}
          {custom && (
            <input
              className="dropdown-custom"
              type="text"
              autoFocus
              placeholder={custom.placeholder}
              aria-label={custom.placeholder}
              // A form field's Enter, as in every settings form here — not a shortcut.
              onKeyDown={(e) => {
                const typed = e.currentTarget.value.trim();
                if (e.key !== "Enter" || !typed) return;
                e.preventDefault();
                custom.onEnter(typed);
                setOpen(false);
              }}
            />
          )}
          {note && <div className="dropdown-note">{note}</div>}
          {options.length === 0 && !custom && (
            <div className="dropdown-empty">{emptyLabel ?? "Nothing here yet"}</div>
          )}
          {options.map((opt) => {
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
        </div>
      )}
    </div>
  );
}
