import { useEffect, useRef, useState, type ReactNode } from "react";
import { MoreVertical } from "lucide-react";
import "./ChatMenu.css";
import { matches } from "../lib/shortcuts";

/**
 * One thing the menu can do to the open chat. `hint` is the line under the
 * label — the place to say why an item is disabled, which is the question a
 * greyed-out row always raises.
 */
export type ChatMenuItem = {
  id: string;
  label: string;
  hint?: string;
  /** The key that does the same, drawn at the row's end. */
  shortcut?: string;
  icon?: ReactNode;
  disabled?: boolean;
  /** Starts a new group: a line above this row. */
  divided?: boolean;
  onSelect: () => void;
};

/**
 * The chat header's "⋮" — actions on the conversation itself, kept out of the
 * header so it stays a title and a branch.
 *
 * A list rather than one button because that is what it is for: exporting is
 * the first of these, and the next one is a row in `items`. The sidebar's
 * rows use it too, for archiving and deleting a chat.
 */
export function ChatMenu({ items }: { items: ChatMenuItem[] }) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);

  // Dismissed the way every other menu in the app is — an outside press or
  // Escape. See `Dropdown`.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (matches(e, "close")) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="chat-menu-wrap" ref={wrap}>
      <button
        type="button"
        className={`iconbtn chat-menu-button${open ? " on" : ""}`}
        title="More"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <MoreVertical size={15} />
      </button>
      {open && (
        <div className="chat-menu" role="menu">
          {items.map((item) => (
            <button
              key={item.id}
              type="button"
              role="menuitem"
              className={`chat-menu-item${item.divided ? " divided" : ""}`}
              disabled={item.disabled}
              onClick={() => {
                setOpen(false);
                item.onSelect();
              }}
            >
              {item.icon && <span className="chat-menu-icon">{item.icon}</span>}
              <span className="chat-menu-text">
                <span>{item.label}</span>
                {item.hint && <span className="hint">{item.hint}</span>}
              </span>
              {item.shortcut && <kbd className="chat-menu-shortcut" aria-hidden>{item.shortcut}</kbd>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
